# Keyboop для Linux (X11)

Порт ядра Keyboop (версия для macOS) на Rust: исправляет слово, набранное не в той раскладке
(`ghbdtn` → `привет`, `руддщ` → `hello`), и переключает раскладку. Детектор перенесён из
`LayoutDetector.swift` один в один, словари и триграммы те же (встроены в бинарник).

Плюс значок в трее и голосовой набор (whisper.cpp на процессоре, всё локально).
Только X11 (на Wayland так перехватывать и печатать нельзя). Перевода и окна настроек нет.

## Что нужно

- **X11-сессия.** Проверить: `echo $XDG_SESSION_TYPE` должно вывести `x11`. На Wayland не работает.
- **Rust 1.85+** (проект на edition 2024). Проще всего через [rustup](https://rustup.rs):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.
- **C/C++-компилятор, CMake и Clang** — whisper.cpp собирается из исходников вместе с проектом.
- **Заголовки ALSA и libxkbcommon-x11**, `pkg-config`, `curl` (им качаются модели).
- **Трей с поддержкой StatusNotifierItem** — в XFCE это стандартная панель (плагин «Status Tray»
  или «Область уведомлений» в свежих версиях); без него программа работает, но без значка.

Зависимости одной командой:

```bash
# Manjaro / Arch
sudo pacman -S --needed base-devel cmake clang pkgconf alsa-lib libxkbcommon-x11 curl

# Debian / Ubuntu / Mint
sudo apt install build-essential cmake clang libclang-dev pkg-config \
                 libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev curl

# Fedora
sudo dnf install gcc-c++ cmake clang-devel pkgconf-pkg-config \
                 alsa-lib-devel libxkbcommon-devel libxkbcommon-x11-devel curl
```

Микрофон читается через ALSA; на системах с PipeWire нужен мост `pipewire-alsa`
(обычно уже стоит).

## Сборка и запуск

```bash
git clone https://github.com/CatsInPartyPants/keyboop-linux.git && cd keyboop-linux
make build                      # cargo build --release, первая сборка ~несколько минут (whisper.cpp)
./target/release/keyboop        # запустить в текущей X-сессии, журнал прямо в терминал
```

Остановить — `Ctrl+C`. Подробный журнал: `RUST_LOG=debug ./target/release/keyboop`.
Тесты: `make test`.

Для голосового набора один раз скачай модель (без неё всё остальное работает):

```bash
./target/release/keyboop download-model small   # ~470 МБ в ~/.local/share/keyboop/models
```

## Установка как сервис (автозапуск)

```bash
make install     # сборка, ~/.local/bin/keyboop, systemd-сервис, автозапуск XFCE, сразу стартует
make logs        # журнал (journalctl --user -u keyboop -f)
make stop        # остановить
make start       # запустить снова (например, после правки настроек)
make uninstall   # удалить (настройки в ~/.config/keyboop остаются)
```

Как это запускается: при входе в XFCE автозапуск передаёт systemd переменные `DISPLAY` и
`XAUTHORITY` и стартует пользовательский сервис `keyboop`. Если программа упадёт, systemd
перезапустит её через 2 секунды.

**Не XFCE** (MATE, Cinnamon, i3 и т. п.): файл `~/.config/autostart/keyboop.desktop` ограничен
строкой `OnlyShowIn=XFCE;` — удали её, и автозапуск заработает в любом окружении, которое
поддерживает XDG Autostart. В оконных менеджерах без него добавь в `~/.xinitrc` или конфиг WM:

```bash
systemctl --user import-environment DISPLAY XAUTHORITY && systemctl --user restart keyboop
```

Если `~/.local/bin` нет в `PATH`, команды вида `keyboop check …` вызывай по полному пути
`~/.local/bin/keyboop`.

## Как пользоваться

| Что | По умолчанию | На ThinkPad без Pause |
|---|---|---|
| Авто-исправление | по пробелу | |
| Исправить текущее/последнее слово | `Pause` | `Fn+P` |
| Отменить авто-исправление (и запомнить слово) | тот же хоткей сразу после него | `Fn+P` |
| Исправить всю фразу | `Shift+Pause` | `Shift+Fn+P` |
| Включить/выключить авто | `Ctrl+Pause` | `Ctrl+Fn+P` |

## Значок в трее

Цвет: коралловый — авто-исправление включено, серый — выключено, красный — идёт запись,
янтарный — распознаю. В меню: авто-исправление, диктовка, настройки, исключения, выход.

## Голосовой набор

Держи **правый Ctrl**, говори, отпусти — текст напечатается туда, где курсор. Правый Ctrl
вместе с другой клавишей (Ctrl+C) — обычное сочетание, запись отменяется; короткий тычок тоже.

Модели лежат в `~/.local/share/keyboop/models`. Скачать: `keyboop download-model small`.
Замер на i5-8350U (без видеокарты), язык задан явно:

| Модель | фраза 3.5 с | фраза 11 с | русский |
|---|---|---|---|
| small (по умолчанию) | ~5.5 с | ~5.9 с | хорошо |
| base | ~1.5 с | ~1.7 с | хуже |

Whisper всегда считает окно в 30 секунд, поэтому короткая фраза стоит почти как длинная;
укорачивать окно нельзя (см. разбор в WhisperBridge.swift). `voice_language = "auto"` вдвое
медленнее: язык определяется отдельным проходом модели.

Проверить без микрофона: `keyboop transcribe запись.wav`.

## Настройки

Настройки: `~/.config/keyboop/config.toml` (создаётся при первом запуске), исключения:
`~/.config/keyboop/exceptions.toml`. После правки: `make start`.

Проверить детектор без запуска:

```bash
keyboop check ghbdtn          # ghbdtn → привет
keyboop check yt привет       # с предыдущим словом: yt → не
```

## Как устроено

- `src/detector.rs` — порт `LayoutDetector.decide` (словарь, триграммы, все спец-правила).
- `src/keymap.rs` — порт `Keymap.swift`; живая таблица строится из XKB (аналог `DynamicKeymap`).
- `src/engine.rs` — модель набранного текста: слова, хвосты пробелов, отмена. Без X11, с тестами.
- `src/voice.rs` — запись (cpal → PipeWire), whisper.cpp, затравка и чистка «призраков» из Swift.
- `src/tray.rs` — значок (StatusNotifierItem через ksni), `src/bus.rs` — сообщения в основной поток.
- `src/x11.rs` — XInput2 raw events (чтение), XTest (печать), XKB (раскладка), XGrabKey (хоткеи).
  Свои синтетические нажатия отсекаются по устройству-источнику (XTEST).

Ограничения первой версии:
- авто-исправление срабатывает только на пробеле: Enter в чатах уже отправил сообщение, а Tab часто
  переводит фокус, поэтому после них контекст просто сбрасывается;
- правки на лету посреди слова (live-fix) пока нет;
- клик мышью, стрелки и сочетания с Ctrl/Alt сбрасывают контекст: где теперь каретка, неизвестно.

Данные `data/words_*.json`, `data/trigrams_*.json` — из проекта keyswitcher (MIT, © 2026 Ilya Granin).
`data/extra_words.json` генерируется `tools/extract_extra_words.py` из `ExtraWords.swift`.
