//! Native menu bar (macOS: the application's `NSMenu` main menu; Windows: the window's menu
//! bar) built with `muda` from the toolkit-neutral menu model of `markise_ui_egui::menu`.
//!
//! - Items are keyed by [`Item::key`]; chosen items arrive through muda's event handler, which
//!   queues the key and requests a repaint (no polling: idle stays at zero frames, NFR-10).
//!   The app drains the queue at the start of its frame ([`MenuHost::take_actions`]).
//! - [`MenuHost::sync`] runs after each frame. [`MenuSync`] diffs the model against what the
//!   native menus show: an unchanged model makes no native call, enabled/checked/label changes
//!   are applied to the existing items, and the native menus are rebuilt only when the
//!   structure changes (theme list, new registry commands).
//! - While a menu is open (macOS: `menu_tracking`) nothing is changed at all — replacing the
//!   main menu dismisses an open menu (issue #81); held-back changes are applied after it
//!   closed.
//! - macOS draws and routes the key equivalents itself, so the menu performs a shortcut before
//!   egui sees the key (same action as the egui shortcut path). On Windows the accelerators
//!   are shown only: muda's accelerator table needs `TranslateAcceleratorW` in the message
//!   loop, which is not installed, so keys keep reaching egui (no double dispatch).
//! - AppKit inserts its own "Enter/Exit Full Screen" item into any menu titled "View"; the
//!   model therefore has no Full Screen item on macOS (the app's shortcut path keeps ⌃⌘F for
//!   the case the system item does not answer it). Do not add one here.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use markise_ui_egui::MenuHost;
use markise_ui_egui::menu::{Check, Entry, Item, Menu, MenuAction, MenuBar, MenuKind, Role, Trigger, accelerator_text};
use markise_ui_egui::menu_sync::{MenuOp, MenuSync};
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, IsMenuItem, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

enum Handle {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

pub struct NativeMenu {
    menu: Option<muda::Menu>,
    /// What the native menus show (diffs the model each frame).
    shown: MenuSync,
    items: HashMap<String, Handle>,
    actions: HashMap<String, MenuAction>,
    /// Submenus in depth-first order (the order of [`MenuOp::SubmenuEnabled`]).
    submenus: Vec<Submenu>,
    rx: Receiver<(String, Trigger)>,
    #[cfg(windows)]
    hwnd: isize,
}

impl NativeMenu {
    /// Prepares the native menu bar for the window of `cc`; menus appear with the first
    /// [`MenuHost::sync`]. `None` if the platform window is not available.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<NativeMenu> {
        #[cfg(windows)]
        let hwnd = {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match cc.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => h.hwnd.get(),
                _ => return None,
            }
        };
        let (tx, rx) = channel();
        let ctx = cc.egui_ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send((e.id.0, trigger()));
            ctx.request_repaint();
        }));
        #[cfg(target_os = "macos")]
        crate::menu_tracking::install(&cc.egui_ctx);
        Some(NativeMenu {
            menu: None,
            shown: MenuSync::new(cfg!(target_os = "macos")),
            items: HashMap::new(),
            actions: HashMap::new(),
            submenus: Vec::new(),
            rx,
            #[cfg(windows)]
            hwnd,
        })
    }

    fn rebuild(&mut self, bar: &MenuBar) {
        self.items.clear();
        self.actions.clear();
        self.submenus.clear();
        let menu = muda::Menu::new();
        for m in &bar.menus {
            if !self.shown.shows(m) {
                continue;
            }
            let sub = self.submenu(m);
            let _ = menu.append(&sub);
            #[cfg(target_os = "macos")]
            match m.kind {
                MenuKind::Window => sub.set_as_windows_menu_for_nsapp(),
                MenuKind::Help => sub.set_as_help_menu_for_nsapp(),
                _ => {}
            }
        }
        #[cfg(target_os = "macos")]
        menu.init_for_nsapp();
        #[cfg(windows)]
        // SAFETY: `hwnd` is the app's live top-level window (from the creation context); the
        // menu is attached to it on the UI thread that owns it, and the previous menu is
        // detached first.
        unsafe {
            if let Some(old) = &self.menu {
                let _ = old.remove_for_hwnd(self.hwnd);
            }
            if let Err(e) = menu.init_for_hwnd(self.hwnd) {
                eprintln!("markise: native menu bar failed: {e}");
            }
        }
        self.menu = Some(menu);
    }

    fn submenu(&mut self, m: &Menu) -> Submenu {
        let sub = Submenu::new(esc(&m.title), m.enabled);
        self.submenus.push(sub.clone());
        for e in &m.entries {
            match e {
                Entry::Item(i) => {
                    let h = self.item(i);
                    let _ = match &h {
                        Handle::Plain(x) => sub.append(x),
                        Handle::Check(x) => sub.append(x),
                    };
                    self.actions.insert(i.key.clone(), i.action.clone());
                    self.items.insert(i.key.clone(), h);
                }
                Entry::Separator => {
                    let _ = sub.append(&PredefinedMenuItem::separator());
                }
                Entry::Header(h) => {
                    let _ = sub.append(&MenuItem::new(esc(h), false, None));
                }
                Entry::Submenu(child) => {
                    let c = self.submenu(child);
                    let _ = sub.append(&c);
                }
                Entry::Role(r) => {
                    if let Some(p) = role(*r) {
                        let _ = sub.append(&p as &dyn IsMenuItem);
                    }
                }
            }
        }
        sub
    }

    fn item(&self, i: &Item) -> Handle {
        let accel = i
            .chord
            .as_ref()
            .and_then(accelerator_text)
            .and_then(|t| t.parse::<Accelerator>().ok());
        match i.check {
            Check::None => Handle::Plain(MenuItem::with_id(i.key.as_str(), esc(&i.label), i.enabled, accel)),
            Check::Toggle(on) | Check::Radio(on) => Handle::Check(CheckMenuItem::with_id(
                i.key.as_str(),
                esc(&i.label),
                i.enabled,
                on,
                accel,
            )),
        }
    }

    /// Applies in-place changes (anything but [`MenuOp::Rebuild`]).
    fn apply(&mut self, op: &MenuOp) {
        match op {
            MenuOp::Rebuild => {}
            MenuOp::Label { key, label } => match self.items.get(key) {
                Some(Handle::Plain(x)) => x.set_text(esc(label)),
                Some(Handle::Check(x)) => x.set_text(esc(label)),
                None => {}
            },
            MenuOp::Enabled { key, enabled } => match self.items.get(key) {
                Some(Handle::Plain(x)) => x.set_enabled(*enabled),
                Some(Handle::Check(x)) => x.set_enabled(*enabled),
                None => {}
            },
            MenuOp::Checked { key, checked } => {
                if let Some(Handle::Check(x)) = self.items.get(key) {
                    x.set_checked(*checked);
                }
            }
            MenuOp::SubmenuEnabled { index, enabled } => {
                if let Some(sub) = self.submenus.get(*index) {
                    sub.set_enabled(*enabled);
                }
            }
        }
    }
}

/// How the current menu event was triggered: on macOS the item's key equivalent while the
/// application handles a key-down event, else a click. Windows menus never route
/// accelerators here (see the module comment), so they are always clicks.
fn trigger() -> Trigger {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        const NS_EVENT_TYPE_KEY_DOWN: usize = 10;
        let Some(cls) = AnyClass::get(c"NSApplication") else {
            return Trigger::Click;
        };
        // SAFETY: muda calls the handler on the main thread from the menu item's action;
        // `sharedApplication`, `currentEvent` and `type` exist with these return types.
        unsafe {
            let app: *mut AnyObject = msg_send![cls, sharedApplication];
            let Some(app) = app.as_ref() else { return Trigger::Click };
            let ev: *mut AnyObject = msg_send![app, currentEvent];
            if let Some(ev) = ev.as_ref() {
                let ty: usize = msg_send![ev, type];
                if ty == NS_EVENT_TYPE_KEY_DOWN {
                    return Trigger::Shortcut;
                }
            }
        }
    }
    Trigger::Click
}

/// muda treats `&` as a mnemonic marker (Windows access keys; stripped on macOS): a literal
/// `&` ("Font & Size…") is written `&&`.
fn esc(label: &str) -> String {
    label.replace('&', "&&")
}

fn role(r: Role) -> Option<PredefinedMenuItem> {
    Some(match r {
        Role::About => PredefinedMenuItem::about(
            Some("About Markise"),
            Some(muda::AboutMetadata {
                name: Some("Markise".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..Default::default()
            }),
        ),
        Role::Services => PredefinedMenuItem::services(None),
        Role::Hide => PredefinedMenuItem::hide(Some("Hide Markise")),
        Role::HideOthers => PredefinedMenuItem::hide_others(None),
        Role::ShowAll => PredefinedMenuItem::show_all(None),
        Role::Minimize => PredefinedMenuItem::minimize(None),
        Role::Zoom => PredefinedMenuItem::maximize(Some("Zoom")),
        Role::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
    })
}

impl MenuHost for NativeMenu {
    fn take_actions(&mut self) -> Vec<(MenuAction, Trigger)> {
        self.rx
            .try_iter()
            .filter_map(|(key, t)| Some((self.actions.get(&key).cloned()?, t)))
            .collect()
    }

    fn sync(&mut self, bar: &MenuBar) {
        #[cfg(target_os = "macos")]
        let tracking = crate::menu_tracking::is_tracking();
        // Windows: menu tracking is not observed (issue #81 was reported on macOS)
        #[cfg(not(target_os = "macos"))]
        let tracking = false;
        let items = &self.items;
        // against the native state: a native check item flips itself when it is clicked
        let native = |key: &str| match items.get(key) {
            Some(Handle::Check(x)) => Some(x.is_checked()),
            _ => None,
        };
        let ops = self.shown.plan(bar, tracking, native);
        for op in &ops {
            if *op == MenuOp::Rebuild {
                self.rebuild(bar);
            } else {
                self.apply(op);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use markise_ui_egui::menu::{Platform, accelerator_text};

    #[test]
    fn ampersands_are_escaped_for_muda() {
        assert_eq!(super::esc("Font & Size…"), "Font && Size…");
    }

    /// Every shortcut of the menu model (both platforms) is expressible as a native accelerator.
    #[test]
    fn model_accelerators_parse_as_native_accelerators() {
        let mut app = markise_ui_egui::MarkiseApp::new(Default::default());
        for p in [Platform::Mac, Platform::Other] {
            app.set_menu_platform(p);
            for i in app.menu_model().items() {
                if let Some(c) = &i.chord {
                    let t = accelerator_text(c).unwrap_or_else(|| panic!("{}: {c:?}", i.key));
                    assert!(t.parse::<muda::accelerator::Accelerator>().is_ok(), "{}: {t}", i.key);
                }
            }
        }
    }
}
