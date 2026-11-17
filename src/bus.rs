//! Сообщения в основной поток. Трей и распознавание речи живут в своих потоках, а печатать
//! и менять состояние может только основной цикл X11. Он спит в poll() на сокете X-сервера,
//! поэтому кроме канала нужен «будильник» — второй сокет, в который пишем байт.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    /// Модели нет на диске.
    NoModel,
    Downloading,
    Loading,
    Idle,
    Recording,
    Transcribing,
    /// Голос выключен в настройках.
    Off,
}

#[derive(Debug)]
pub enum Msg {
    ToggleAuto,
    /// Пункт меню «Начать/остановить диктовку».
    ToggleDictation,
    DownloadModel,
    OpenConfig,
    OpenExceptions,
    Quit,
    VoiceStatus(VoiceStatus),
    VoiceText(String),
    VoiceError(String),
}

#[derive(Clone)]
pub struct Bus {
    tx: mpsc::Sender<Msg>,
    wake: Arc<UnixStream>,
}

pub struct BusRx {
    rx: mpsc::Receiver<Msg>,
    pub wake: UnixStream,
}

pub fn channel() -> std::io::Result<(Bus, BusRx)> {
    let (a, b) = UnixStream::pair()?;
    a.set_nonblocking(true)?;
    b.set_nonblocking(true)?;
    let (tx, rx) = mpsc::channel();
    Ok((Bus { tx, wake: Arc::new(a) }, BusRx { rx, wake: b }))
}

impl Bus {
    pub fn send(&self, m: Msg) {
        if self.tx.send(m).is_ok() {
            let _ = (&*self.wake).write(&[1]);
        }
    }
}

impl BusRx {
    /// Забрать все накопившиеся сообщения (и опустошить будильник).
    pub fn drain(&mut self) -> Vec<Msg> {
        let mut buf = [0u8; 64];
        while matches!(self.wake.read(&mut buf), Ok(n) if n > 0) {}
        self.rx.try_iter().collect()
    }
}
