//! Значок в трее (StatusNotifierItem — панель XFCE 4.16+ показывает его в «Области уведомлений»).
//!
//! Цвет плашки — состояние: коралловый — авто-исправление включено, серый — выключено,
//! красный — идёт запись, янтарный — распознаём. Внутри — знак Keyboop из macOS-версии.

use crate::bus::{Bus, Msg, VoiceStatus};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Icon, MenuItem, ToolTip};
use std::sync::OnceLock;

pub struct KbTray {
    pub bus: Bus,
    pub auto: bool,
    pub voice: VoiceStatus,
    pub voice_hotkey: String,
    pub model: String,
    pub last: String,
}

const CORAL: [u8; 3] = [0xFF, 0x7A, 0x59];
const GRAY: [u8; 3] = [0x8A, 0x8A, 0x8A];
const RED: [u8; 3] = [0xE5, 0x39, 0x35];
const AMBER: [u8; 3] = [0xF5, 0xA6, 0x23];

/// Знак 128×128, только альфа-канал.
fn mark() -> &'static (usize, Vec<u8>) {
    static M: OnceLock<(usize, Vec<u8>)> = OnceLock::new();
    M.get_or_init(|| {
        let dec = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../data/menubar-mark.png")[..]));
        let mut reader = dec.read_info().expect("menubar-mark.png");
        let mut buf = vec![0; reader.output_buffer_size().unwrap_or(128 * 128 * 4)];
        let info = reader.next_frame(&mut buf).expect("menubar-mark.png");
        let n = (info.width * info.height) as usize;
        let alpha = (0..n).map(|i| buf[i * 4 + 3]).collect();
        (info.width as usize, alpha)
    })
}

fn render(size: usize, bg: [u8; 3]) -> Icon {
    let (src_w, alpha) = mark();
    let mut data = Vec::with_capacity(size * size * 4);
    let r = size as f32 * 0.22; // радиус скругления
    let pad = size as f32 * 0.14; // поля вокруг знака
    let inner = size as f32 - 2.0 * pad;
    for y in 0..size {
        for x in 0..size {
            // Плашка со скруглёнными углами (с простым сглаживанием по расстоянию).
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = fx.clamp(r, size as f32 - r);
            let cy = fy.clamp(r, size as f32 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let bg_a = (r - d + 0.5).clamp(0.0, 1.0);
            // Знак: среднее по квадрату исходных пикселей, попадающему в этот пиксель.
            let mut mark_a = 0.0;
            let (mx, my) = ((fx - pad) / inner, (fy - pad) / inner);
            if (0.0..1.0).contains(&mx) && (0.0..1.0).contains(&my) {
                let step = *src_w as f32 / inner;
                let (sx0, sy0) = ((mx * *src_w as f32) as usize, (my * *src_w as f32) as usize);
                let k = step.ceil().max(1.0) as usize;
                let (mut sum, mut cnt) = (0u32, 0u32);
                for yy in sy0..(sy0 + k).min(*src_w) {
                    for xx in sx0..(sx0 + k).min(*src_w) {
                        sum += alpha[yy * src_w + xx] as u32;
                        cnt += 1;
                    }
                }
                if cnt > 0 {
                    mark_a = sum as f32 / cnt as f32 / 255.0;
                }
            }
            let mix = |c: u8| ((c as f32) * (1.0 - mark_a) + 255.0 * mark_a) as u8;
            data.extend_from_slice(&[(bg_a * 255.0) as u8, mix(bg[0]), mix(bg[1]), mix(bg[2])]);
        }
    }
    Icon { width: size as i32, height: size as i32, data }
}

impl KbTray {
    fn color(&self) -> [u8; 3] {
        match self.voice {
            VoiceStatus::Recording => RED,
            VoiceStatus::Transcribing => AMBER,
            _ if self.auto => CORAL,
            _ => GRAY,
        }
    }

    fn voice_line(&self) -> String {
        match self.voice {
            VoiceStatus::Off => "Голосовой набор выключен".into(),
            VoiceStatus::NoModel => format!("Нет модели распознавания «{}»", self.model),
            VoiceStatus::Downloading => format!("Скачиваю модель «{}»…", self.model),
            VoiceStatus::Loading => "Загружаю модель распознавания…".into(),
            VoiceStatus::Idle => format!("Диктовка: держи {}", self.voice_hotkey),
            VoiceStatus::Recording => "Идёт запись…".into(),
            VoiceStatus::Transcribing => "Распознаю…".into(),
        }
    }
}

impl ksni::Tray for KbTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "keyboop".into()
    }

    fn title(&self) -> String {
        "Keyboop".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let c = self.color();
        [22, 32, 48].into_iter().map(|s| render(s, c)).collect()
    }

    fn tool_tip(&self) -> ToolTip {
        let mut desc = format!(
            "Авто-исправление {}\n{}",
            if self.auto { "включено" } else { "выключено" },
            self.voice_line()
        );
        if !self.last.is_empty() {
            desc.push_str(&format!("\n{}", self.last));
        }
        ToolTip { title: "Keyboop".into(), description: desc, ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem { label: self.voice_line(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Авто-исправление раскладки".into(),
                checked: self.auto,
                activate: Box::new(|t: &mut Self| t.bus.send(Msg::ToggleAuto)),
                ..Default::default()
            }
            .into(),
        ];
        match self.voice {
            VoiceStatus::Idle | VoiceStatus::Recording => items.push(
                StandardItem {
                    label: if self.voice == VoiceStatus::Recording { "Остановить диктовку" } else { "Начать диктовку" }.into(),
                    activate: Box::new(|t: &mut Self| t.bus.send(Msg::ToggleDictation)),
                    ..Default::default()
                }
                .into(),
            ),
            VoiceStatus::NoModel => items.push(
                StandardItem {
                    label: format!("Скачать модель «{}»", self.model),
                    activate: Box::new(|t: &mut Self| t.bus.send(Msg::DownloadModel)),
                    ..Default::default()
                }
                .into(),
            ),
            _ => {}
        }
        items.extend([
            MenuItem::Separator,
            StandardItem {
                label: "Настройки…".into(),
                activate: Box::new(|t: &mut Self| t.bus.send(Msg::OpenConfig)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Исключения…".into(),
                activate: Box::new(|t: &mut Self| t.bus.send(Msg::OpenExceptions)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Выход".into(),
                activate: Box::new(|t: &mut Self| t.bus.send(Msg::Quit)),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}
