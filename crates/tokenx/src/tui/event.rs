use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, KeyEvent, KeyEventKind, MouseEvent};

pub enum Event {
    Tick,
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    BackgroundReady,
    InputError(io::Error),
}

pub struct EventHandler {
    rx: mpsc::Receiver<Event>,
    tx: mpsc::Sender<Event>,
}

fn should_forward_key_event(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn read_terminal_event(
    tick_rate: Duration,
    poll: impl FnOnce(Duration) -> io::Result<bool>,
    read: impl FnOnce() -> io::Result<event::Event>,
) -> io::Result<Option<Event>> {
    if !poll(tick_rate)? {
        return Ok(Some(Event::Tick));
    }
    Ok(match read()? {
        event::Event::Key(key) if should_forward_key_event(&key) => Some(Event::Key(key)),
        event::Event::Key(_) => None,
        event::Event::Mouse(mouse) => Some(Event::Mouse(mouse)),
        event::Event::Resize(width, height) => Some(Event::Resize(width, height)),
        // Ignored focus and paste events still give background work a turn.
        _ => Some(Event::Tick),
    })
}

impl EventHandler {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let event_tx = tx.clone();

        thread::spawn(move || loop {
            match read_terminal_event(tick_rate, event::poll, event::read) {
                Ok(Some(event)) => {
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = event_tx.send(Event::InputError(error));
                    break;
                }
            }
        });

        Self { rx, tx }
    }

    pub(super) fn sender(&self) -> mpsc::Sender<Event> {
        self.tx.clone()
    }

    pub fn next(&mut self) -> Result<Event> {
        Ok(self.rx.recv()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn forwards_press_and_repeat_but_not_release() {
        assert!(should_forward_key_event(&key(KeyEventKind::Press)));
        assert!(should_forward_key_event(&key(KeyEventKind::Repeat)));
        assert!(!should_forward_key_event(&key(KeyEventKind::Release)));
    }

    #[test]
    fn poll_failure_is_returned_without_reading_or_emitting_a_tick() {
        let result = read_terminal_event(
            Duration::ZERO,
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "poll failed",
                ))
            },
            || panic!("read must not follow failed polling"),
        );
        let error = result
            .err()
            .expect("poll failure must reach the event owner");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "poll failed");
    }

    #[test]
    fn read_failure_is_returned_instead_of_an_ignored_event_tick() {
        let result = read_terminal_event(
            Duration::ZERO,
            |_| Ok(true),
            || Err(io::Error::new(io::ErrorKind::BrokenPipe, "read failed")),
        );
        let error = result
            .err()
            .expect("read failure must reach the event owner");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(error.to_string(), "read failed");
    }

    #[test]
    fn idle_polling_and_ignored_focus_events_remain_ticks() {
        assert!(matches!(
            read_terminal_event(
                Duration::ZERO,
                |_| Ok(false),
                || panic!("idle poll must not read")
            ),
            Ok(Some(Event::Tick))
        ));
        for event in [event::Event::FocusGained, event::Event::FocusLost] {
            assert!(matches!(
                read_terminal_event(Duration::ZERO, |_| Ok(true), || Ok(event)),
                Ok(Some(Event::Tick))
            ));
        }
    }
}
