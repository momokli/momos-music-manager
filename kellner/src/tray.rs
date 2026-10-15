//! Tray / menu-bar widget (issue #368, plan phase P5).
//!
//! The plan makes the tray/menu-bar widget **v1**, not a later add-on, on top of
//! the `launchd` / `systemd` / Task Scheduler loop. Its right-click menu shows
//! **exactly** two entries — [`MENU_LABELS`]: `feierabend` (quit the loop) and
//! `re-auth` (re-run pairing). There is **no** other device configuration here.
//!
//! The whole widget is behind the off-by-default cargo feature `tray`, so the
//! Linux CI (and any headless host) builds `kellner` without GTK/appindicator
//! system libraries. Only [`MENU_LABELS`] is always compiled, so the contract is
//! testable without a GUI.

/// The tray menu labels, in order: quit, then re-auth.
#[allow(dead_code)] // only read by the `tray` feature and by tests.
pub const MENU_LABELS: [&str; 2] = ["feierabend", "re-auth"];

#[cfg(feature = "tray")]
mod imp {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use anyhow::Result;

    use super::MENU_LABELS;

    /// Run the tray/menu-bar widget until "feierabend" is chosen or `stop` is set.
    ///
    /// `on_reauth` is invoked when the user picks "re-auth" (the caller wires it
    /// to the pairing flow). On macOS the caller must drive this from the main
    /// thread so the native run loop is alive; on Linux/GTK the GTK main loop is
    /// pumped by `tray-icon`.
    pub fn run(stop: Arc<AtomicBool>, on_reauth: impl Fn() + Send + 'static) -> Result<()> {
        use tray_icon::TrayIconBuilder;
        use tray_icon::menu::{Menu, MenuEvent, MenuItem};

        let menu = Menu::new();
        let quit_item = MenuItem::new(MENU_LABELS[0], true, None);
        let reauth_item = MenuItem::new(MENU_LABELS[1], true, None);
        menu.append_items(&[&quit_item, &reauth_item])?;

        let _tray = TrayIconBuilder::new()
            .with_tooltip("kellner")
            .with_menu(Box::new(menu))
            .build()?;

        let quit_id = quit_item.id().clone();
        let reauth_id = reauth_item.id().clone();
        let events = MenuEvent::receiver();

        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            match events.recv_timeout(Duration::from_millis(500)) {
                Ok(event) if event.id == quit_id => break,
                Ok(event) if event.id == reauth_id => on_reauth(),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(feature = "tray")]
pub use imp::run;

#[cfg(test)]
mod tests {
    use super::MENU_LABELS;

    #[test]
    fn menu_has_exactly_feierabend_and_reauth() {
        assert_eq!(MENU_LABELS, ["feierabend", "re-auth"]);
    }
}
