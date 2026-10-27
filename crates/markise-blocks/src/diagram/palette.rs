//! Diagram colors derived from the active color scheme (ADR 0007: colors come from the
//! `ColorScheme` part of the composed theme; diagrams have no colors of their own).

use markise_theme::{Rgba, Theme};

use super::scene::{Paint, Role};

/// Resolved colors for every [`Role`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiagramPalette {
    colors: [Rgba; ROLES],
}

const ROLES: usize = 22;

fn idx(r: Role) -> usize {
    r as usize
}

/// `a` blended towards `b` by `t` (0 = a, 1 = b), in sRGB.
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Rgba([
        ch(a.0[0], b.0[0]),
        ch(a.0[1], b.0[1]),
        ch(a.0[2], b.0[2]),
        ch(a.0[3], b.0[3]),
    ])
}

impl DiagramPalette {
    pub fn from_theme(t: &Theme) -> Self {
        // an (almost) transparent page counts as white/black by the scheme's darkness
        let page = if t.page_bg.0[3] < 128 {
            if t.dark {
                Rgba::hex(0x1e1e1e)
            } else {
                Rgba::hex(0xffffff)
            }
        } else {
            t.page_bg.with_alpha(255)
        };
        let text = t.text.with_alpha(255);
        let accent = t.chrome.accent.with_alpha(255);
        let warn = t.alert[3].with_alpha(255);
        let mut c = [text; ROLES];
        let mut set = |r: Role, v: Rgba| c[idx(r)] = v;
        set(Role::NodeFill, mix(page, accent, 0.13));
        set(Role::NodeStroke, mix(page, accent, 0.7));
        set(Role::NodeText, text);
        set(Role::EdgeStroke, mix(page, text, 0.72));
        set(Role::EdgeText, text);
        set(Role::EdgeLabelFill, page);
        set(Role::ClusterFill, mix(page, text, 0.04));
        set(Role::ClusterStroke, mix(page, text, 0.32));
        set(Role::ClusterText, text);
        set(Role::NoteFill, mix(page, warn, 0.16));
        set(Role::NoteStroke, mix(page, warn, 0.6));
        set(Role::NoteText, text);
        set(Role::ActorFill, mix(page, accent, 0.13));
        set(Role::ActorStroke, mix(page, accent, 0.7));
        set(Role::ActorText, text);
        set(Role::LifeLine, mix(page, text, 0.45));
        set(Role::ActivationFill, mix(page, text, 0.1));
        set(Role::FrameStroke, mix(page, text, 0.55));
        set(Role::FrameLabelFill, mix(page, text, 0.08));
        set(Role::FrameText, text);
        set(Role::Text, text);
        set(Role::Background, page);
        DiagramPalette { colors: c }
    }

    pub fn role(&self, r: Role) -> Rgba {
        self.colors[idx(r)]
    }

    pub fn resolve(&self, p: Paint) -> Rgba {
        match p {
            Paint::Role(r) => self.role(r),
            Paint::Custom(c) => c,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_fit_the_table() {
        assert_eq!(Role::Background as usize, ROLES - 1);
    }

    #[test]
    fn node_text_is_legible_in_light_and_dark() {
        for t in [Theme::light(), Theme::dark()] {
            let p = DiagramPalette::from_theme(&t);
            for (fg, bg) in [
                (Role::NodeText, Role::NodeFill),
                (Role::EdgeText, Role::EdgeLabelFill),
                (Role::NoteText, Role::NoteFill),
                (Role::ActorText, Role::ActorFill),
                (Role::ClusterText, Role::ClusterFill),
            ] {
                let c = p.role(fg).contrast(p.role(bg));
                assert!(c >= 4.5, "{} {fg:?} on {bg:?}: {c:.2}", t.name);
            }
            assert!(
                p.role(Role::EdgeStroke).contrast(p.role(Role::Background)) >= 3.0,
                "{}",
                t.name
            );
        }
    }
}
