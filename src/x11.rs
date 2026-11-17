//! Связка с X11.
//!
//! * Нажатия читаем через XInput2 raw events на корневом окне: видим ввод во всех программах
//!   без захвата клавиатуры, а свои синтетические нажатия отсекаем по устройству-источнику
//!   (XTEST-клавиатура — отдельное slave-устройство).
//! * Символ нажатия декодируем libxkbcommon по живому XKB-состоянию (раскладка, Shift, Caps).
//! * Исправление печатаем через XTest: Backspace × N, затем текст клавишами нужной раскладки.
//! * Раскладку переключаем XkbLatchLockState (то же, что делает Alt+Shift).
//! * Горячие клавиши: XGrabKey только чтобы клавиша не доходила до программ; само срабатывание
//!   ловим по raw-событию, чтобы порядок с остальным вводом был строгим.

use crate::bus::{self, Bus, BusRx, Msg, VoiceStatus};
use crate::config::Config;
use crate::translate::{self, Translate};
use crate::tray::KbTray;
use crate::voice::{self, Voice, VoiceConfig};
use crate::engine::{Action, Engine, Input};
use crate::exceptions::Exceptions;
use crate::keymap::{self, Table};

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::os::fd::AsRawFd;
use std::time::Instant;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xkb::{self as xxkb, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::xcb_ffi::XCBConnection;
use xkbcommon::xkb;

type Res<T> = Result<T, Box<dyn Error>>;

const MOD_SHIFT: u16 = 1;
const MOD_LOCK: u16 = 2;
const MOD_CTRL: u16 = 4;
const MOD_ALT: u16 = 8;
const MOD_NUM: u16 = 16;
const MOD_SUPER: u16 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HotkeyKind {
    Word,
    Phrase,
    Toggle,
    Translate,
}

#[derive(Clone, Debug)]
struct Hotkey {
    kind: HotkeyKind,
    keysym: xkb::Keysym,
    mods: u16,
    keycode: Option<u8>,
}

fn parse_hotkey(kind: HotkeyKind, s: &str) -> Option<Hotkey> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut mods = 0;
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    for m in &parts[..parts.len() - 1] {
        mods |= match m.to_lowercase().as_str() {
            "shift" => MOD_SHIFT,
            "ctrl" | "control" => MOD_CTRL,
            "alt" | "mod1" => MOD_ALT,
            "super" | "win" | "mod4" => MOD_SUPER,
            other => {
                log::error!("хоткей «{s}»: неизвестный модификатор «{other}»");
                return None;
            }
        };
    }
    let name = parts[parts.len() - 1];
    let mut keysym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
    if keysym.raw() == 0 {
        keysym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    }
    if keysym.raw() == 0 {
        log::error!("хоткей «{s}»: неизвестная клавиша «{name}» (имена как в xev)");
        return None;
    }
    Some(Hotkey { kind, keysym, mods, keycode: None })
}

/// Где на клавиатуре лежит символ.
#[derive(Clone, Copy, Debug)]
struct KeyPos {
    keycode: u8,
    group: u32,
    shift: bool,
}

struct Kb {
    keymap: xkb::Keymap,
    state: xkb::State,
    lat_group: Option<u32>,
    cyr_group: Option<u32>,
    /// Символ → все места, где он набирается без AltGr.
    positions: HashMap<char, Vec<KeyPos>>,
    backspace: Option<u8>,
    modifier_keycodes: HashSet<u8>,
}

fn sym_char(keymap: &xkb::Keymap, kc: u32, group: u32, level: u32) -> Option<char> {
    let syms = keymap.key_get_syms_by_level(kc.into(), group, level);
    let sym = *syms.first()?;
    char::from_u32(xkb::keysym_to_utf32(sym)).filter(|c| *c != '\0' && !c.is_control())
}

fn is_modifier_sym(sym: xkb::Keysym) -> bool {
    let r = sym.raw();
    // Shift_L … Hyper_R, ISO_Lock … ISO_Last_Group_Lock, Mode_switch
    (0xffe1..=0xffee).contains(&r) || (0xfe01..=0xfe0f).contains(&r) || r == 0xff7e
}

impl Kb {
    fn new(conn: &XCBConnection, ctx: &xkb::Context, dev: i32) -> Self {
        let keymap = xkb::x11::keymap_new_from_device(ctx, conn, dev, xkb::KEYMAP_COMPILE_NO_FLAGS);
        let state = xkb::x11::state_new_from_device(&keymap, conn, dev);
        let mut lat_group = None;
        let mut cyr_group = None;
        // Классифицируем раскладки по клавише «A» (AC01, keycode 38 в evdev).
        let probe = keymap.key_by_name("AC01").map(|k| k.raw()).unwrap_or(38);
        for g in 0..keymap.num_layouts() {
            match sym_char(&keymap, probe, g, 0) {
                Some(c) if ('\u{0400}'..='\u{04FF}').contains(&c) && cyr_group.is_none() => cyr_group = Some(g),
                Some(c) if c.is_ascii_alphabetic() && lat_group.is_none() => lat_group = Some(g),
                _ => {}
            }
            log::info!("раскладка {g}: {}", keymap.layout_get_name(g));
        }
        let mut positions: HashMap<char, Vec<KeyPos>> = HashMap::new();
        let mut backspace = None;
        let mut modifier_keycodes = HashSet::new();
        let (min, max) = (keymap.min_keycode().raw(), keymap.max_keycode().raw().min(255));
        for level in 0..2u32 {
            for kc in min..=max {
                for g in 0..keymap.num_layouts_for_key(kc.into()) {
                    if let Some(c) = sym_char(&keymap, kc, g, level) {
                        positions.entry(c).or_default().push(KeyPos { keycode: kc as u8, group: g, shift: level == 1 });
                    }
                    if level == 0 {
                        if let Some(&sym) = keymap.key_get_syms_by_level(kc.into(), g, 0).first() {
                            if sym.raw() == 0xff08 && backspace.is_none() {
                                backspace = Some(kc as u8);
                            }
                            if is_modifier_sym(sym) {
                                modifier_keycodes.insert(kc as u8);
                            }
                        }
                    }
                }
            }
        }
        let kb = Kb { keymap, state, lat_group, cyr_group, positions, backspace, modifier_keycodes };
        kb.install_live_tables();
        kb
    }

    /// Живые таблицы символов «латинская ↔ кириллическая раскладка» (аналог DynamicKeymap).
    fn install_live_tables(&self) {
        let (Some(lat), Some(cyr)) = (self.lat_group, self.cyr_group) else {
            log::warn!("не нашёл пару латинская+кириллическая раскладка — работаю по встроенной таблице US/ЙЦУКЕН");
            keymap::set_live_tables(Table::new(), Table::new());
            return;
        };
        let (mut en, mut ru) = (Table::new(), Table::new());
        let (min, max) = (self.keymap.min_keycode().raw(), self.keymap.max_keycode().raw().min(255));
        for level in 0..2u32 {
            for kc in min..=max {
                if let (Some(a), Some(b)) = (sym_char(&self.keymap, kc, lat, level), sym_char(&self.keymap, kc, cyr, level)) {
                    if a != b {
                        en.entry(a).or_insert(b);
                        ru.entry(b).or_insert(a);
                    }
                }
            }
        }
        log::info!("таблица раскладок: {} пар", en.len());
        keymap::set_live_tables(en, ru);
    }

    fn current_group(&self) -> u32 {
        self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE)
    }

    fn current_is_cyrillic(&self) -> bool {
        Some(self.current_group()) == self.cyr_group
    }

    fn active_mods(&self) -> u16 {
        let on = |n: &str| self.state.mod_name_is_active(n, xkb::STATE_MODS_DEPRESSED | xkb::STATE_MODS_LATCHED);
        let mut m = 0;
        if on(xkb::MOD_NAME_SHIFT) {
            m |= MOD_SHIFT;
        }
        if on(xkb::MOD_NAME_CTRL) {
            m |= MOD_CTRL;
        }
        if on(xkb::MOD_NAME_ALT) {
            m |= MOD_ALT;
        }
        if on(xkb::MOD_NAME_LOGO) {
            m |= MOD_SUPER;
        }
        m
    }

    fn caps_locked(&self) -> bool {
        self.state.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_LOCKED)
    }

    fn keycode_for(&self, keysym: xkb::Keysym) -> Option<u8> {
        let (min, max) = (self.keymap.min_keycode().raw(), self.keymap.max_keycode().raw().min(255));
        (min..=max).find_map(|kc| {
            (0..self.keymap.num_layouts_for_key(kc.into()))
                .any(|g| self.keymap.key_get_syms_by_level(kc.into(), g, 0).first() == Some(&keysym))
                .then_some(kc as u8)
        })
    }

    fn position(&self, c: char, prefer_group: u32) -> Option<KeyPos> {
        let v = self.positions.get(&c)?;
        v.iter().find(|p| p.group == prefer_group).or_else(|| v.first()).copied()
    }
}

struct App {
    conn: XCBConnection,
    root: xproto::Window,
    ctx: xkb::Context,
    dev: i32,
    kb: Kb,
    engine: Engine,
    hotkeys: Vec<Hotkey>,
    xtest_devices: HashSet<u16>,
    pressed: HashMap<u8, Instant>,
    repeats_seen: bool,
    exclude_apps: Vec<String>,
    atom_active: xproto::Atom,
    atom_wm_class: xproto::Atom,
    cfg: Config,
    bus: Bus,
    bus_rx: BusRx,
    tray: Option<ksni::blocking::Handle<KbTray>>,
    voice: Option<Voice>,
    voice_status: VoiceStatus,
    voice_keysym: Option<xkb::Keysym>,
    /// Клавиша диктовки зажата: когда нажата и была ли с ней другая клавиша (тогда это сочетание).
    voice_held: Option<(Instant, bool)>,
    translate: Option<Translate>,
    /// Невидимое окно, в которое X-сервер кладёт содержимое выделения (PRIMARY).
    sel_win: xproto::Window,
    atoms: SelAtoms,
    quit: bool,
}

#[derive(Clone, Copy)]
struct SelAtoms {
    primary: xproto::Atom,
    utf8: xproto::Atom,
    prop: xproto::Atom,
    incr: xproto::Atom,
}

pub fn run() -> Res<()> {
    let cfg = Config::load();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
        return Err("сессия Wayland: эта версия работает только в X11".into());
    }
    let (conn, screen_num) = XCBConnection::connect(None).map_err(|e| format!("нет X-дисплея ({e}); DISPLAY задан?"))?;
    let root = conn.setup().roots[screen_num].root;

    // XKB: через x11rb (чтобы он знал номер расширения для разбора событий) и через xkbcommon.
    conn.xkb_use_extension(1, 0)?.reply()?;
    let (mut maj, mut min, mut ev, mut er) = (0, 0, 0, 0);
    if !xkb::x11::setup_xkb_extension(&conn, 1, 0, xkb::x11::SetupXkbExtensionFlags::NoFlags, &mut maj, &mut min, &mut ev, &mut er) {
        return Err("нет расширения XKB".into());
    }
    let dev = xkb::x11::get_core_keyboard_device_id(&conn);
    let events = xxkb::EventType::NEW_KEYBOARD_NOTIFY | xxkb::EventType::MAP_NOTIFY | xxkb::EventType::STATE_NOTIFY;
    conn.xkb_select_events(
        xxkb::ID::USE_CORE_KBD.into(),
        xxkb::EventType::from(0u16),
        events,
        xxkb::MapPart::from(0xffu16),
        xxkb::MapPart::from(0xffu16),
        &xxkb::SelectEventsAux::new(),
    )?
    .check()?;

    // XInput2 ≥ 2.2: raw-события доставляются всегда, даже при чужом захвате клавиатуры.
    let v = conn.xinput_xi_query_version(2, 2)?.reply()?;
    if (v.major_version, v.minor_version) < (2, 2) {
        return Err(format!("нужен XInput 2.2, есть {}.{}", v.major_version, v.minor_version).into());
    }
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: 1, // XIAllMasterDevices
            mask: vec![xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE | xinput::XIEventMask::RAW_BUTTON_PRESS],
        }],
    )?
    .check()?;

    let mut xtest_devices = HashSet::new();
    for d in conn.xinput_xi_query_device(0u16)?.reply()?.infos {
        if String::from_utf8_lossy(&d.name).contains("XTEST") {
            xtest_devices.insert(d.deviceid);
        }
    }
    log::info!("XTEST-устройства (свой ввод, игнорируем): {xtest_devices:?}");

    conn.change_window_attributes(root, &xproto::ChangeWindowAttributesAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE))?
        .check()?;
    let atom_active = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom;
    let atom_wm_class = conn.intern_atom(false, b"WM_CLASS")?.reply()?.atom;
    let atoms = SelAtoms {
        primary: xproto::AtomEnum::PRIMARY.into(),
        utf8: conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom,
        prop: conn.intern_atom(false, b"KEYBOOP_SELECTION")?.reply()?.atom,
        incr: conn.intern_atom(false, b"INCR")?.reply()?.atom,
    };
    let sel_win = conn.generate_id()?;
    conn.create_window(
        0,
        sel_win,
        root,
        -10,
        -10,
        1,
        1,
        0,
        xproto::WindowClass::INPUT_ONLY,
        0,
        &xproto::CreateWindowAux::new(),
    )?
    .check()?;

    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let kb = Kb::new(&conn, &ctx, dev);

    let _ = crate::data::data(); // словари грузим заранее, не на первом пробеле
    let mut engine = Engine::new(Exceptions::load());
    engine.auto = cfg.auto;
    engine.log_text = cfg.log_text;

    let hotkeys = [
        (HotkeyKind::Word, &cfg.hotkey_word),
        (HotkeyKind::Phrase, &cfg.hotkey_phrase),
        (HotkeyKind::Toggle, &cfg.hotkey_toggle),
        (HotkeyKind::Translate, &cfg.hotkey_translate),
    ]
    .into_iter()
    .filter_map(|(k, s)| parse_hotkey(k, s))
    .collect();

    let (bus, bus_rx) = bus::channel()?;
    let voice_keysym = if cfg.voice {
        let k = xkb::keysym_from_name(cfg.voice_hotkey.trim(), xkb::KEYSYM_NO_FLAGS);
        if k.raw() == 0 {
            log::error!("клавиша диктовки «{}» не найдена (имена как в xev)", cfg.voice_hotkey);
            None
        } else {
            Some(k)
        }
    } else {
        None
    };
    let voice = cfg.voice.then(|| {
        let threads = if cfg.voice_threads > 0 {
            cfg.voice_threads
        } else {
            // Замер на i5-8350U: 8 потоков чуть быстрее 4.
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8) as i32
        };
        Voice::spawn(
            VoiceConfig { model: cfg.voice_model.clone(), language: cfg.voice_language.clone(), threads },
            bus.clone(),
        )
    });
    let translate = (!cfg.hotkey_translate.trim().is_empty()).then(|| Translate::spawn(bus.clone(), 4));
    let voice_status = if cfg.voice { VoiceStatus::Loading } else { VoiceStatus::Off };
    let tray = {
        use ksni::blocking::TrayMethods;
        KbTray {
            bus: bus.clone(),
            auto: engine.auto,
            voice: voice_status,
            voice_hotkey: human_key(&cfg.voice_hotkey),
            model: cfg.voice_model.clone(),
            last: String::new(),
        }
        .spawn()
        .map_err(|e| log::warn!("значок в трее недоступен: {e}"))
        .ok()
    };

    let mut app = App {
        conn,
        root,
        ctx,
        dev,
        kb,
        engine,
        hotkeys,
        xtest_devices,
        pressed: HashMap::new(),
        repeats_seen: false,
        exclude_apps: cfg.exclude_apps.iter().map(|s| s.to_lowercase()).collect(),
        atom_active,
        atom_wm_class,
        cfg,
        bus,
        bus_rx,
        tray,
        voice,
        voice_status,
        voice_keysym,
        voice_held: None,
        translate,
        sel_win,
        atoms,
        quit: false,
    };
    app.grab_hotkeys();
    app.conn.flush()?;
    log::info!("Keyboop запущен, авто-исправление {}", if app.engine.auto { "включено" } else { "выключено" });

    let xfd = app.conn.as_raw_fd();
    let wfd = app.bus_rx.wake.as_raw_fd();
    while !app.quit {
        while let Some(event) = app.conn.poll_for_event()? {
            if let Err(e) = app.handle(event) {
                log::warn!("ошибка обработки события: {e}");
            }
        }
        for m in app.bus_rx.drain() {
            if let Err(e) = app.message(m) {
                log::warn!("ошибка обработки сообщения: {e}");
            }
        }
        app.conn.flush()?;
        if app.quit {
            break;
        }
        let mut fds = [
            libc::pollfd { fd: xfd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wfd, events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: массив pollfd живёт на стеке на всё время вызова.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if r < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
        if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            return Err("X-сервер закрыл соединение".into());
        }
    }
    log::info!("выход по команде из меню");
    Ok(())
}

/// «Control_R» → «правый Ctrl» для меню.
fn human_key(name: &str) -> String {
    match name.trim() {
        "Control_R" => "правый Ctrl".into(),
        "Control_L" => "левый Ctrl".into(),
        "Alt_R" => "правый Alt".into(),
        "Alt_L" => "левый Alt".into(),
        "Super_L" | "Super_R" => "Win".into(),
        other => other.into(),
    }
}

fn play_sound(name: &str) {
    let path = format!("/usr/share/sounds/freedesktop/stereo/{name}.oga");
    let _ = std::process::Command::new("pw-play")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

impl App {
    fn is_voice_key(&self, kc: u8) -> bool {
        let Some(vk) = self.voice_keysym else { return false };
        let (min, max) = (self.kb.keymap.min_keycode().raw(), self.kb.keymap.max_keycode().raw());
        (min..=max).contains(&(kc as u32))
            && (0..self.kb.keymap.num_layouts_for_key((kc as u32).into()))
                .any(|g| self.kb.keymap.key_get_syms_by_level((kc as u32).into(), g, 0).first() == Some(&vk))
    }

    /// true — нажатие съедено диктовкой.
    fn voice_key_press(&mut self, kc: u8, repeat: bool) -> bool {
        if self.is_voice_key(kc) {
            if !repeat && self.voice_held.is_none() && self.voice_status == VoiceStatus::Idle {
                if let Some(v) = &self.voice {
                    v.send(voice::Cmd::Start);
                    self.voice_held = Some((Instant::now(), false));
                    if self.cfg.voice_sounds {
                        play_sound("device-added");
                    }
                }
            }
            // Сама клавиша — модификатор, тексту она не мешает; дальше её не разбираем.
            return self.voice_held.is_some();
        }
        if let Some((_, other)) = self.voice_held.as_mut() {
            if !*other {
                // Правый Ctrl + клавиша = обычное сочетание (Ctrl+C), а не диктовка.
                *other = true;
                if let Some(v) = &self.voice {
                    v.send(voice::Cmd::Cancel);
                }
            }
        }
        false
    }

    fn voice_key_release(&mut self, kc: u8) {
        if !self.is_voice_key(kc) {
            return;
        }
        let Some((t, other)) = self.voice_held.take() else { return };
        let Some(v) = &self.voice else { return };
        if other {
            return;
        }
        if t.elapsed().as_millis() < 350 {
            v.send(voice::Cmd::Cancel); // короткий тычок — не диктовка
            return;
        }
        v.send(voice::Cmd::Stop);
        if self.cfg.voice_sounds {
            play_sound("device-removed");
        }
    }

    fn update_tray(&self, f: impl FnOnce(&mut KbTray) + Send) {
        if let Some(t) = &self.tray {
            t.update(f);
        }
    }

    fn open_file(path: std::path::PathBuf) {
        let _ = std::process::Command::new("xdg-open")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn message(&mut self, m: Msg) -> Res<()> {
        match m {
            Msg::ToggleAuto => self.hotkey(HotkeyKind::Toggle)?,
            Msg::ToggleDictation => {
                if let Some(v) = &self.voice {
                    match self.voice_status {
                        VoiceStatus::Idle => {
                            v.send(voice::Cmd::Start);
                            if self.cfg.voice_sounds {
                                play_sound("device-added");
                            }
                        }
                        VoiceStatus::Recording => {
                            v.send(voice::Cmd::Stop);
                            if self.cfg.voice_sounds {
                                play_sound("device-removed");
                            }
                        }
                        _ => {}
                    }
                }
            }
            Msg::DownloadModel => {
                let name = self.cfg.voice_model.clone();
                let bus = self.bus.clone();
                let v = self.voice.clone();
                self.bus.send(Msg::VoiceStatus(VoiceStatus::Downloading));
                std::thread::spawn(move || match voice::download_model(&name) {
                    Ok(p) => {
                        log::info!("модель скачана: {}", p.display());
                        if let Some(v) = v {
                            v.send(voice::Cmd::Reload);
                        }
                    }
                    Err(e) => {
                        bus.send(Msg::VoiceError(format!("не скачал модель: {e}")));
                        bus.send(Msg::VoiceStatus(VoiceStatus::NoModel));
                    }
                });
            }
            Msg::OpenConfig => Self::open_file(crate::config::config_dir().join("config.toml")),
            Msg::OpenExceptions => {
                let p = Exceptions::path();
                if !p.exists() {
                    self.engine.exc.save();
                }
                Self::open_file(p)
            }
            Msg::Quit => self.quit = true,
            Msg::VoiceStatus(s) => {
                self.voice_status = s;
                self.update_tray(move |t| t.voice = s);
            }
            Msg::VoiceError(e) => {
                log::error!("{e}");
                notify(&e);
            }
            Msg::Translated { dir, source, text } => translate::show(dir, &source, &text),
            Msg::TranslateError(e) => {
                log::error!("{e}");
                notify(&e);
            }
            Msg::VoiceText(text) => {
                let mut t = voice::normalize_for_typing(&text, self.cfg.voice_final_period, self.cfg.voice_capitalize);
                if t.is_empty() {
                    return Ok(());
                }
                // Диктуем сразу после набранного слова — отделяем пробелом.
                if self.engine.has_current_word() {
                    t.insert(0, ' ');
                }
                log::info!("диктовка: печатаю {} симв.", t.chars().count());
                self.replace(0, &t, None)?;
                self.engine.input(Input::Reset);
            }
        }
        Ok(())
    }

    fn grab_hotkeys(&mut self) {
        let _ = self.conn.ungrab_key(0u8, self.root, xproto::ModMask::ANY);
        for i in 0..self.hotkeys.len() {
            let kc = self.kb.keycode_for(self.hotkeys[i].keysym);
            self.hotkeys[i].keycode = kc;
            let hk = self.hotkeys[i].clone();
            let Some(kc) = kc else {
                log::warn!("хоткей {:?}: клавиши {} нет на этой клавиатуре", hk.kind, xkb::keysym_get_name(hk.keysym));
                continue;
            };
            let mut ok = true;
            for extra in [0, MOD_LOCK, MOD_NUM, MOD_LOCK | MOD_NUM] {
                let r = self
                    .conn
                    .grab_key(true, self.root, (hk.mods | extra).into(), kc, xproto::GrabMode::ASYNC, xproto::GrabMode::ASYNC)
                    .map_err(|e| e.to_string())
                    .and_then(|c| c.check().map_err(|e| format!("{e:?}")));
                if let Err(e) = r {
                    ok = false;
                    log::warn!("хоткей {:?}: не смог захватить ({e}) — его держит другая программа?", hk.kind);
                    break;
                }
            }
            if ok {
                log::info!("хоткей {:?}: {}{}", hk.kind, mods_name(hk.mods), xkb::keysym_get_name(hk.keysym));
            }
        }
    }

    fn rebuild_keyboard(&mut self) {
        self.kb = Kb::new(&self.conn, &self.ctx, self.dev);
        self.grab_hotkeys();
        let _ = self.conn.flush();
    }

    fn handle(&mut self, event: Event) -> Res<()> {
        match event {
            Event::XkbStateNotify(e) => {
                self.kb.state.update_mask(
                    u16::from(e.base_mods).into(),
                    u16::from(e.latched_mods).into(),
                    u16::from(e.locked_mods).into(),
                    e.base_group as u32,
                    e.latched_group as u32,
                    u8::from(e.locked_group).into(),
                );
            }
            Event::XkbNewKeyboardNotify(_) | Event::XkbMapNotify(_) => {
                // Эти события приходят и тогда, когда ведущая клавиатура переключается между
                // физической и XTEST-клавиатурой (то есть после каждой нашей перепечатки), хотя
                // раскладки не менялись. Перечитываем, только если keymap действительно другой.
                let fresh = xkb::x11::keymap_new_from_device(&self.ctx, &self.conn, self.dev, xkb::KEYMAP_COMPILE_NO_FLAGS);
                if fresh.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1) != self.kb.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1) {
                    log::info!("раскладки изменились — перечитываю");
                    self.rebuild_keyboard();
                }
            }
            Event::XinputRawKeyPress(e) => {
                if self.xtest_devices.contains(&e.sourceid) {
                    return Ok(());
                }
                let kc = e.detail as u8;
                let repeat = u32::from(e.flags) & u32::from(xinput::KeyEventFlags::KEY_REPEAT) != 0;
                if repeat {
                    self.repeats_seen = true;
                }
                self.pressed.entry(kc).or_insert_with(Instant::now);
                if self.voice_key_press(kc, repeat) {
                    return Ok(());
                }
                self.key_press(kc)?;
            }
            Event::XinputRawKeyRelease(e) => {
                if self.xtest_devices.contains(&e.sourceid) {
                    return Ok(());
                }
                let kc = e.detail as u8;
                self.voice_key_release(kc);
                if let Some(t) = self.pressed.remove(&kc) {
                    // Автоповтор, которого мы не видели (raw-повторы приходят не везде): клавишу держали
                    // дольше задержки повтора — сколько символов ушло, неизвестно. Сбрасываем контекст.
                    if !self.repeats_seen && !self.kb.modifier_keycodes.contains(&kc) && t.elapsed().as_millis() > 450 {
                        self.engine.input(Input::Reset);
                    }
                }
            }
            Event::XinputRawButtonPress(e) => {
                if (1..=3).contains(&e.detail) && !self.xtest_devices.contains(&e.sourceid) {
                    self.engine.input(Input::Click);
                }
            }
            Event::SelectionNotify(e) if e.requestor == self.sel_win => self.selection_ready(e.property)?,
            Event::PropertyNotify(e) if e.atom == self.atom_active => {
                self.engine.input(Input::Focus);
                self.engine.excluded_app = self.active_window_excluded();
            }
            _ => {}
        }
        Ok(())
    }

    fn key_press(&mut self, kc: u8) -> Res<()> {
        let mods = self.kb.active_mods();
        // Горячая клавиша (сравниваем без учёта Caps/NumLock).
        if let Some(hk) = self.hotkeys.iter().find(|h| h.keycode == Some(kc) && h.mods == mods).cloned() {
            return self.hotkey(hk.kind);
        }
        let sym = self.kb.state.key_get_one_sym(kc.into());
        if is_modifier_sym(sym) {
            return Ok(());
        }
        let raw = sym.raw();
        let input = match raw {
            0xff08 => Input::Backspace { word: mods & (MOD_CTRL | MOD_ALT) != 0 },
            _ if mods & (MOD_CTRL | MOD_ALT | MOD_SUPER) != 0 => Input::Reset,
            0x0020 => Input::Space,
            0xff09 | 0xfe20 | 0xff0d | 0xff8d => Input::Break, // Tab, ISO_Left_Tab, Return, KP_Enter
            // Стрелки/Home/End/PgUp/PgDn (0xff50–0xff58), Esc, Delete, Insert, keypad-навигация
            0xff50..=0xff58 | 0xff1b | 0xffff | 0xff63 | 0xff95..=0xff9f => Input::Reset,
            _ => {
                let s = self.kb.state.key_get_utf8(kc.into());
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if !c.is_control() => Input::Char(c),
                    _ => return Ok(()), // F-клавиши, мультимедиа и прочее — тексту не мешают
                }
            }
        };
        let action = self.engine.input(input);
        self.apply(action)
    }

    fn hotkey(&mut self, kind: HotkeyKind) -> Res<()> {
        let cyr = self.kb.current_is_cyrillic();
        let action = match kind {
            HotkeyKind::Word => self.engine.manual_word(cyr),
            HotkeyKind::Phrase => self.engine.manual_phrase(cyr),
            HotkeyKind::Toggle => {
                self.engine.auto = !self.engine.auto;
                log::info!("авто-исправление {}", if self.engine.auto { "включено" } else { "выключено" });
                notify(if self.engine.auto { "Авто-исправление включено" } else { "Авто-исправление выключено" });
                let auto = self.engine.auto;
                self.update_tray(move |t| t.auto = auto);
                Action::None
            }
            HotkeyKind::Translate => {
                // Просим владельца выделения положить текст в свойство нашего окна;
                // ответ придёт событием SelectionNotify.
                let a = self.atoms;
                self.conn.convert_selection(self.sel_win, a.primary, a.utf8, a.prop, x11rb::CURRENT_TIME)?;
                Action::None
            }
        };
        self.apply(action)
    }

    fn lock_group(&self, group: u32) -> Res<()> {
        self.conn.xkb_latch_lock_state(
            xxkb::ID::USE_CORE_KBD.into(),
            xproto::ModMask::from(0u16),
            xproto::ModMask::from(0u16),
            true,
            xxkb::Group::from(group as u8),
            xproto::ModMask::from(0u16),
            false,
            0,
        )?;
        Ok(())
    }

    fn set_caps(&self, on: bool) -> Res<()> {
        self.conn.xkb_latch_lock_state(
            xxkb::ID::USE_CORE_KBD.into(),
            xproto::ModMask::LOCK,
            if on { xproto::ModMask::LOCK } else { xproto::ModMask::from(0u16) },
            false,
            xxkb::Group::from(0u8),
            xproto::ModMask::from(0u16),
            false,
            0,
        )?;
        Ok(())
    }

    fn fake(&self, kc: u8, press: bool) -> Res<()> {
        let t = if press { xproto::KEY_PRESS_EVENT } else { xproto::KEY_RELEASE_EVENT };
        self.conn.xtest_fake_input(t, kc, 0, self.root, 0, 0, 0)?;
        Ok(())
    }

    fn tap(&self, kc: u8, shift: Option<u8>) -> Res<()> {
        if let Some(s) = shift {
            self.fake(s, true)?;
        }
        self.fake(kc, true)?;
        self.fake(kc, false)?;
        if let Some(s) = shift {
            self.fake(s, false)?;
        }
        Ok(())
    }

    fn apply(&mut self, action: Action) -> Res<()> {
        match action {
            Action::None => Ok(()),
            Action::ToggleLayout => {
                let next = if self.kb.current_is_cyrillic() { self.kb.lat_group } else { self.kb.cyr_group };
                if let Some(g) = next {
                    self.lock_group(g)?;
                    self.conn.flush()?;
                }
                Ok(())
            }
            Action::Replace { delete, text, layout_cyrillic } => self.replace(delete, &text, layout_cyrillic),
        }
    }

    fn replace(&mut self, delete: usize, text: &str, layout_cyrillic: Option<bool>) -> Res<()> {
        let Some(bs) = self.kb.backspace else { return Err("на клавиатуре нет BackSpace".into()) };
        let shift_kc = self.kb.keycode_for(xkb::Keysym::from(0xffe1u32)); // Shift_L
        let start_group = self.kb.current_group();
        let target_group = match layout_cyrillic {
            Some(true) => self.kb.cyr_group,
            Some(false) => self.kb.lat_group,
            None => None,
        }
        .unwrap_or(start_group);

        // Отпускаем ВСЕ физически зажатые клавиши. Модификаторы (Ctrl от хоткея, Shift) испортили бы
        // перепечатку, а ещё не отпущенный пробел — тот самый, что запустил авто-исправление, — сервер
        // не даст нажать повторно: наш пробел потеряется и слова слипнутся (баг из первой проверки).
        let held: Vec<u8> = self.pressed.keys().copied().collect();
        for k in &held {
            self.fake(*k, false)?;
            self.pressed.remove(k);
        }
        let caps = self.kb.caps_locked();
        if caps {
            self.set_caps(false)?;
        }
        if target_group != start_group {
            self.lock_group(target_group)?;
        }
        self.conn.sync()?;

        for _ in 0..delete {
            self.tap(bs, None)?;
        }
        let mut group = target_group;
        for c in text.chars() {
            let Some(p) = self.kb.position(c, group) else {
                log::warn!("символ U+{:04X} не набирается ни в одной раскладке — пропускаю", c as u32);
                continue;
            };
            if p.group != group {
                // Символа нет в текущей раскладке: временно переключаемся (с синхронизацией,
                // чтобы смена раскладки и нажатия не перемешались).
                self.conn.sync()?;
                self.lock_group(p.group)?;
                self.conn.sync()?;
                group = p.group;
            }
            let shift = if p.shift { shift_kc } else { None };
            self.tap(p.keycode, shift)?;
        }
        if group != target_group {
            self.conn.sync()?;
            self.lock_group(target_group)?;
        }
        if caps {
            self.set_caps(true)?;
        }
        self.conn.sync()?;
        Ok(())
    }

    /// Выделение пришло (или его нет). Отдаём текст в поток перевода.
    fn selection_ready(&mut self, prop: xproto::Atom) -> Res<()> {
        let Some(tr) = &self.translate else { return Ok(()) };
        if prop == x11rb::NONE {
            notify("Нет выделенного текста");
            return Ok(());
        }
        // Берём до 64 КБ (длина в 32-битных словах); свойство сразу удаляем.
        let r = self.conn.get_property(true, self.sel_win, prop, xproto::AtomEnum::ANY, 0, 16 * 1024)?.reply()?;
        if r.type_ == self.atoms.incr {
            notify("Выделение слишком большое для перевода");
            return Ok(());
        }
        let text = String::from_utf8_lossy(&r.value).trim().to_string();
        if text.is_empty() {
            notify("Нет выделенного текста");
            return Ok(());
        }
        let n = text.chars().count();
        if n > translate::MAX_CHARS {
            notify(&format!("Слишком длинный текст: {n} симв. (не больше {})", translate::MAX_CHARS));
            return Ok(());
        }
        if self.cfg.log_text {
            log::info!("перевожу: {text:?}");
        } else {
            log::info!("перевожу {n} симв.");
        }
        tr.send(text);
        Ok(())
    }

    fn active_window_excluded(&self) -> bool {
        if self.exclude_apps.is_empty() {
            return false;
        }
        let get = || -> Res<bool> {
            let r = self.conn.get_property(false, self.root, self.atom_active, xproto::AtomEnum::WINDOW, 0, 1)?.reply()?;
            let Some(win) = r.value32().and_then(|mut v| v.next()) else { return Ok(false) };
            if win == 0 {
                return Ok(false);
            }
            let r = self.conn.get_property(false, win, self.atom_wm_class, xproto::AtomEnum::STRING, 0, 256)?.reply()?;
            let classes: Vec<String> =
                r.value.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).to_lowercase()).collect();
            let hit = classes.iter().any(|c| self.exclude_apps.contains(c));
            if hit {
                log::debug!("окно {classes:?} в исключениях — авто молчит");
            }
            Ok(hit)
        };
        get().unwrap_or(false)
    }
}

fn mods_name(m: u16) -> String {
    let mut s = String::new();
    for (bit, n) in [(MOD_CTRL, "ctrl+"), (MOD_ALT, "alt+"), (MOD_SHIFT, "shift+"), (MOD_SUPER, "super+")] {
        if m & bit != 0 {
            s.push_str(n);
        }
    }
    s
}

fn notify(text: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "Keyboop", "-t", "1500", "Keyboop", text])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

trait SyncExt {
    fn sync(&self) -> Res<()>;
}

impl SyncExt for XCBConnection {
    /// Полный круг до сервера: всё отправленное до этого гарантированно обработано.
    fn sync(&self) -> Res<()> {
        self.get_input_focus()?.reply()?;
        Ok(())
    }
}

#[allow(dead_code)]
fn _assert_traits(c: &XCBConnection) {
    let _ = c.extension_information(xxkb::X11_EXTENSION_NAME);
}
