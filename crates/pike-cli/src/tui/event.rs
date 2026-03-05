use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};

pub(crate) enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Tick,
}

pub(crate) struct EventHandler {
    tick_rate: Duration,
}

impl EventHandler {
    pub(crate) fn new(tick_rate: Duration) -> Self {
        Self { tick_rate }
    }

    pub(crate) fn next(&self) -> std::io::Result<Event> {
        if event::poll(self.tick_rate)? {
            match event::read()? {
                CrosstermEvent::Key(key) => Ok(Event::Key(key)),
                CrosstermEvent::Mouse(mouse) => Ok(Event::Mouse(mouse)),
                _ => Ok(Event::Tick),
            }
        } else {
            Ok(Event::Tick)
        }
    }
}
