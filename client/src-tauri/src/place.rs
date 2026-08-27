//! Where the window was left, and how it gets back there.
//!
//! `tauri.conf.json` gives the window a size and no position, so every window
//! this process builds is placed by Windows — and Windows cascades: four
//! rebuilds in one run landed at `104,104`, `208,208`, `52,52` and `130,130`
//! (DR-38). Task 4.6 turned closing the window into destroying it, which means
//! a person who puts goodvoice where they want it and closes it to the tray
//! gets a *new* window somewhere else, and over a session it walks down the
//! screen.
//!
//! So the rectangle is remembered. It rides in the same file as the chosen
//! server ([`crate::home`]) because that file exists, is written whole, and is
//! already read before the first window exists.
//!
//! # Logical pixels, not physical
//!
//! [`Placement`] is in logical pixels because that is what the window config
//! is in: `WindowConfig::x` and `y` reach `tao` as a `LogicalPosition` and
//! `width`/`height` as a `LogicalSize`. Storing physical would mean converting
//! on the way out with a scale factor we would have to guess — the one of
//! whatever monitor the window is about to land on — so the round trip is kept
//! in the unit that needs no guess.
//!
//! # Two ways back, because there are two windows
//!
//! A window **rebuilt** by the tray is born in place: [`crate::tray`] hands the
//! rectangle to the builder. The **first** window of a run cannot be — it is
//! built from the config before `setup` runs — so [`restore`] moves it, which
//! is invisible only because the window is hidden until it has painted
//! (DR-38). Doing it the other way round for the rebuild would show as a jump:
//! `tray-flicker.ps1` counts every rectangle a window has, hidden ones
//! included, and a window that is built in one place and moved to another has
//! two.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager as _, Window};

use crate::home::Home;

/// A window's outer position and inner size, in logical pixels.
///
/// Outer position and *inner* size because that is the pair the window config
/// names, and mixing the two would grow or shrink the window by the width of
/// its frame on every trip through here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// The left edge of the window frame.
    pub x: f64,
    /// The top edge of the window frame.
    pub y: f64,
    /// The width of the area the webview gets.
    pub width: f64,
    /// The height of the area the webview gets.
    pub height: f64,
    /// Whether it was left filling its screen. The four numbers above are then
    /// the rectangle a person restoring it down should get back, not the one
    /// on screen — a maximised window's own rectangle is the monitor's.
    #[serde(default)]
    pub maximized: bool,
}

/// A screen, reduced to what deciding this needs: a rectangle in logical
/// pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// The left edge of the usable area, taskbar excluded.
    pub x: f64,
    /// The top edge of the usable area.
    pub y: f64,
    /// How wide the usable area is.
    pub width: f64,
    /// How tall the usable area is.
    pub height: f64,
}

/// How much of the window's width has to be on a screen for the position to be
/// one a person can use.
const GRABBABLE_WIDTH: f64 = 96.0;

/// How much of the window's height, measured from its top edge, has to be on a
/// screen — enough title bar to take hold of and drag the rest into view.
const GRABBABLE_HEIGHT: f64 = 32.0;

impl Placement {
    /// Whether a window at this rectangle would be somewhere a person could
    /// reach it.
    ///
    /// The monitor a window was left on is not always there next time: a
    /// laptop comes back from a dock, a second screen is unplugged, a
    /// resolution changes. A remembered position that now lands off every
    /// screen is worse than no memory at all, because the window is *there* —
    /// it exists, it has focus, and nothing is on screen.
    ///
    /// The test is deliberately about the title bar rather than about area.
    /// A window overlapping a screen only along its bottom edge is drawn but
    /// cannot be dragged anywhere, which is the same problem with extra steps.
    ///
    /// With no screens to check against — a host that will not enumerate them
    /// — the answer is yes: losing a good position to a question nobody could
    /// answer is the worse failure.
    #[must_use]
    pub fn is_reachable_on(&self, screens: &[Screen]) -> bool {
        if ![self.x, self.y, self.width, self.height]
            .iter()
            .all(|number| number.is_finite())
        {
            return false;
        }
        if self.width < 1.0 || self.height < 1.0 {
            return false;
        }
        if screens.is_empty() {
            return true;
        }
        screens.iter().any(|screen| {
            let across = (self.x + self.width).min(screen.x + screen.width) - self.x.max(screen.x);
            let title_bar_is_on_it =
                self.y >= screen.y && self.y + GRABBABLE_HEIGHT <= screen.y + screen.height;
            across >= GRABBABLE_WIDTH.min(self.width) && title_bar_is_on_it
        })
    }
}

/// The rectangle to build the next window at, if there is one worth using.
///
/// Both callers — [`restore`] and `tray::open` — ask this rather than the
/// store directly, so the reachability check cannot be forgotten by one of
/// them.
#[must_use]
pub fn remembered(app: &AppHandle) -> Option<Placement> {
    let place = app.state::<Home>().window()?;
    if place.is_reachable_on(&screens(app)) {
        return Some(place);
    }
    eprintln!("the remembered window position is off every screen; letting Windows choose");
    None
}

/// Every monitor's usable area, in logical pixels.
///
/// Work area rather than the whole monitor: a window remembered under the
/// taskbar is a window whose title bar is under the taskbar.
///
/// Each monitor is converted with **its own** scale factor, which is the only
/// thing that makes a mixed-DPI pair of screens comparable to a [`Placement`]
/// at all.
#[must_use]
fn screens(app: &AppHandle) -> Vec<Screen> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            Screen {
                x: f64::from(area.position.x) / scale,
                y: f64::from(area.position.y) / scale,
                width: f64::from(area.size.width) / scale,
                height: f64::from(area.size.height) / scale,
            }
        })
        .collect()
}

/// Records where the window is now, in memory.
///
/// Called on every move and every resize, which is why it does not touch the
/// disk: dragging a window across a screen is hundreds of events, and a file
/// written on each of them is a file written hundreds of times for one
/// decision. [`keep`] is what writes it, once, when the window goes away.
///
/// **A minimised window has no position.** Windows parks one at `-32000` and
/// reports that faithfully, and the minimise is exactly the event this app
/// answers by destroying the window (`tray::window_event`) — so without this
/// guard every trip to the tray would remember the parking space.
pub fn note(window: &Window) {
    // Every window, from anything that has one: `window_painted` is a command
    // and a command is called by whichever webview invoked it. The viewer
    // (task 5.4) has its own rectangle and no business in this one.
    if window.label() != "main" || window.is_minimized().unwrap_or(false) {
        return;
    }
    let home = window.state::<Home>();
    let maximized = window.is_maximized().unwrap_or(false);

    let mut place = if maximized {
        // The rectangle on screen belongs to the monitor. What is worth
        // keeping is the one from before it was maximised, which is whatever
        // was noted last.
        match home.window() {
            Some(place) => place,
            None => return,
        }
    } else {
        let (Ok(scale), Ok(position), Ok(size)) = (
            window.scale_factor(),
            window.outer_position(),
            window.inner_size(),
        ) else {
            return;
        };
        let position = position.to_logical::<f64>(scale);
        let size = size.to_logical::<f64>(scale);
        Placement {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            maximized: false,
        }
    };
    place.maximized = maximized;
    home.note_window(place);
}

/// Writes what [`note`] has been collecting.
///
/// On the window being destroyed and on the way out of [`crate::tray`]'s quit,
/// which between them are every ordinary end of a window. A process killed
/// outright loses the last move, and that is the right trade for not writing a
/// file on every frame of a drag.
pub fn keep(app: &AppHandle) {
    app.state::<Home>().save();
}

/// Puts the run's first window back where the last run left it.
///
/// Only this one: every window after it is built in place, and see the module
/// docs for why the difference matters.
///
/// Failures are shrugged off. A window that would not move is in the wrong
/// place, which is what it was before any of this existed.
pub fn restore(window: &Window) {
    let Some(place) = remembered(window.app_handle()) else {
        return;
    };
    if let Err(error) = window.set_position(LogicalPosition::new(place.x, place.y)) {
        eprintln!("the window would not go back where it was: {error}");
        return;
    }
    if let Err(error) = window.set_size(LogicalSize::new(place.width, place.height)) {
        eprintln!("the window would not go back to its size: {error}");
    }
    if place.maximized {
        let _ = window.maximize();
    }
}

#[cfg(test)]
mod tests {
    use super::{Placement, Screen, GRABBABLE_HEIGHT};

    /// One 1080p screen with a taskbar along the bottom, which is the machine
    /// every other case here is a departure from.
    const ONE_SCREEN: Screen = Screen {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1040.0,
    };

    fn at(x: f64, y: f64) -> Placement {
        Placement {
            x,
            y,
            width: 420.0,
            height: 620.0,
            maximized: false,
        }
    }

    #[test]
    fn a_window_on_the_screen_is_reachable() {
        assert!(at(104.0, 104.0).is_reachable_on(&[ONE_SCREEN]));
        // Hard against the top-left corner, and hanging off the bottom, which
        // a 620-tall window on a short screen legitimately does.
        assert!(at(0.0, 0.0).is_reachable_on(&[ONE_SCREEN]));
        assert!(at(1000.0, 900.0).is_reachable_on(&[ONE_SCREEN]));
    }

    #[test]
    fn the_second_monitor_that_is_not_there_any_more() {
        // Where the window was left while a screen sat to the left of the
        // primary one, which is a negative coordinate and perfectly valid
        // until that screen goes away.
        let left_of_it = Screen {
            x: -1920.0,
            ..ONE_SCREEN
        };
        let place = at(-1500.0, 300.0);
        assert!(place.is_reachable_on(&[ONE_SCREEN, left_of_it]));
        assert!(!place.is_reachable_on(&[ONE_SCREEN]));
    }

    #[test]
    fn a_sliver_on_screen_is_not_enough_to_grab() {
        // Ninety-five pixels of a 420-wide window, which is under the width
        // this asks for and is a window nobody can read.
        assert!(!at(1920.0 - 95.0, 300.0).is_reachable_on(&[ONE_SCREEN]));
        assert!(at(1920.0 - 97.0, 300.0).is_reachable_on(&[ONE_SCREEN]));
    }

    #[test]
    fn a_title_bar_off_the_top_or_the_bottom_is_not_reachable() {
        // Drawn, in both cases, and draggable in neither: what a person takes
        // hold of is the top edge.
        assert!(!at(600.0, -1.0).is_reachable_on(&[ONE_SCREEN]));
        assert!(!at(600.0, 1040.0 - GRABBABLE_HEIGHT + 1.0).is_reachable_on(&[ONE_SCREEN]));
        assert!(at(600.0, 1040.0 - GRABBABLE_HEIGHT).is_reachable_on(&[ONE_SCREEN]));
    }

    #[test]
    fn nonsense_in_the_file_is_not_a_position() {
        let mut place = at(100.0, 100.0);
        place.width = 0.0;
        assert!(!place.is_reachable_on(&[ONE_SCREEN]));

        place = at(f64::NAN, 100.0);
        assert!(!place.is_reachable_on(&[ONE_SCREEN]));

        place = at(100.0, f64::INFINITY);
        assert!(!place.is_reachable_on(&[ONE_SCREEN]));
    }

    #[test]
    fn a_host_that_names_no_screens_is_taken_at_its_word() {
        // Better a position that might be wrong than a window that walks for
        // certain: the check is the answer to a question, not the point.
        assert!(at(104.0, 104.0).is_reachable_on(&[]));
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        let place = Placement {
            x: 104.0,
            y: 208.0,
            width: 420.0,
            height: 620.0,
            maximized: true,
        };
        let text = serde_json::to_string(&place).expect("a placement serialises");
        assert_eq!(
            serde_json::from_str::<Placement>(&text).expect("and comes back"),
            place
        );
        // A file written before this field existed still parses, which is
        // every settings.json on every machine that has run 0.1.0.
        let older: Placement =
            serde_json::from_str(r#"{"x":1.0,"y":2.0,"width":3.0,"height":4.0}"#)
                .expect("an older placement parses");
        assert!(!older.maximized);
    }
}
