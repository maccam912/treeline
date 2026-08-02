//! Buttons on the surrounding web page, wired to the same actions as keys.
//!
//! Touch devices have no keyboard, so the page carries buttons for the actions
//! the desktop build binds to keys. Each button dispatches a custom event that
//! this module listens for and drains once per frame.

use std::cell::Cell;
use std::error::Error;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// One page button and the flag it sets.
struct ButtonListener {
    event: &'static str,
    requested: Rc<Cell<bool>>,
    listener: Closure<dyn FnMut(web_sys::Event)>,
}

impl ButtonListener {
    /// Registers a listener for one custom event on the window.
    fn new(event: &'static str) -> Result<Self, Box<dyn Error>> {
        let window =
            web_sys::window().ok_or_else(|| std::io::Error::other("browser window unavailable"))?;
        let requested = Rc::new(Cell::new(false));
        let flag = Rc::clone(&requested);
        let listener = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            flag.set(true);
        }) as Box<dyn FnMut(web_sys::Event)>);
        window
            .add_event_listener_with_callback(event, listener.as_ref().unchecked_ref())
            .map_err(|error| {
                std::io::Error::other(format!("could not register the {event} button: {error:?}"))
            })?;
        Ok(Self {
            event,
            requested,
            listener,
        })
    }

    fn take(&self) -> bool {
        self.requested.replace(false)
    }
}

impl Drop for ButtonListener {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                self.event,
                self.listener.as_ref().unchecked_ref(),
            );
        }
    }
}

/// The page's action buttons.
pub struct BrowserActions {
    random_warp: ButtonListener,
    water_warp: ButtonListener,
    aerial_mode: ButtonListener,
}

impl BrowserActions {
    /// # Errors
    ///
    /// Returns an error when the page's window is unavailable or a listener
    /// cannot be registered.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            random_warp: ButtonListener::new("treeline-random-warp")?,
            water_warp: ButtonListener::new("treeline-water-warp")?,
            aerial_mode: ButtonListener::new("treeline-toggle-aerial")?,
        })
    }

    pub fn take_random_warp(&self) -> bool {
        self.random_warp.take()
    }

    pub fn take_water_warp(&self) -> bool {
        self.water_warp.take()
    }

    pub fn take_aerial_toggle(&self) -> bool {
        self.aerial_mode.take()
    }

    /// Reflects aerial mode back onto the page button's pressed state.
    pub fn set_aerial_mode_enabled(enabled: bool) {
        let Some(button) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("aerial-mode-button"))
        else {
            return;
        };
        let _ = button.set_attribute("aria-pressed", if enabled { "true" } else { "false" });
    }
}
