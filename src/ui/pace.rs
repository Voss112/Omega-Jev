//! Frame pacing: redraw only as often as something on screen actually moves.

use std::time::Duration;

/// What the window is doing this frame.
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Activity {
    pub focused: bool,
    pub minimized: bool,
    /// A turn is streaming (text, spinners).
    pub busy: bool,
    /// A short-lived animation is on screen (login spinner, benchmark, drag hover).
    pub animating: bool,
    /// Something must be re-checked on a clock (toast expiry, scheduled prompts).
    pub timers: bool,
}

const SMOOTH: Duration = Duration::from_millis(33);
/// Ambient backdrop drift: slow enough that 4 FPS looks continuous.
const AMBIENT: Duration = Duration::from_millis(250);
const CLOCK: Duration = Duration::from_secs(1);

/// Delay before the next unsolicited frame; `None` = wait for input or a runtime event.
pub fn next_frame(a: Activity) -> Option<Duration> {
    if a.minimized {
        return a.timers.then_some(CLOCK);
    }
    if a.busy || a.animating {
        return Some(SMOOTH);
    }
    if a.focused {
        return Some(AMBIENT);
    }
    a.timers.then_some(CLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_pacing_follows_activity() {
        let a = Activity { focused: true, minimized: false, busy: false, animating: false, timers: false };
        assert_eq!(next_frame(a), Some(Duration::from_millis(250)));
        assert_eq!(next_frame(Activity { busy: true, focused: false, ..a }), Some(Duration::from_millis(33)));
        assert_eq!(next_frame(Activity { animating: true, ..a }), Some(Duration::from_millis(33)));
        assert_eq!(next_frame(Activity { focused: false, ..a }), None);
        assert_eq!(next_frame(Activity { focused: false, timers: true, ..a }), Some(Duration::from_secs(1)));
        assert_eq!(next_frame(Activity { minimized: true, busy: true, ..a }), None);
        assert_eq!(next_frame(Activity { minimized: true, timers: true, ..a }), Some(Duration::from_secs(1)));
    }
}
