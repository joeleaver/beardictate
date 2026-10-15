//! The tray glyph, drawn from the same shapes as `design/logo/tray-32-*.svg`.
//!
//! State lives in the snout colour and a corner dot, so the silhouette never
//! changes (`design/canvas/Tray.dc.html`). Drawn in code, supersampled, so
//! there is no SVG renderer to ship and every state is one function call.

use crate::engine::State;
use crate::settings::TrayTheme;

/// Pixels per side of the drawn icon. The tray scales it; 64 stays sharp
/// on 2x displays.
pub const SIZE: u32 = 64;

/// Which way round the glyph's colours go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taskbar {
    /// A light taskbar: a navy bear.
    Light,
    /// A dark taskbar: an ice-blue bear.
    Dark,
}

impl Taskbar {
    pub fn resolve(theme: TrayTheme) -> Taskbar {
        match theme {
            TrayTheme::Light => Taskbar::Light,
            TrayTheme::Dark => Taskbar::Dark,
            TrayTheme::Auto => guess(),
        }
    }
}

/// KDE: how bright the colour scheme's window background is (the panel
/// follows it with the default Plasma style). Elsewhere, and when unsure:
/// dark, the default for Windows 11, GNOME and most KDE setups' panels.
fn guess() -> Taskbar {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")));
    let globals = config.and_then(|c| std::fs::read_to_string(c.join("kdeglobals")).ok());
    globals.as_deref().and_then(kde_taskbar).unwrap_or(Taskbar::Dark)
}

/// Reads `[Colors:Window] BackgroundNormal=r,g,b` from `kdeglobals`.
fn kde_taskbar(globals: &str) -> Option<Taskbar> {
    let section = globals.split("\n[").find(|s| s.trim_start_matches('[').starts_with("Colors:Window]"))?;
    let rgb = section.lines().find_map(|l| l.strip_prefix("BackgroundNormal="))?;
    let c: Vec<f32> = rgb.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    let [r, g, b] = c[..] else { return None };
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    Some(if luma < 128.0 { Taskbar::Dark } else { Taskbar::Light })
}

type Rgb = [u8; 3];

const NAVY: Rgb = [0x0E, 0x1A, 0x30];
const ICE: Rgb = [0xDD, 0xE8, 0xFB];
const COBALT: Rgb = [0x2F, 0x6F, 0xDB];

struct Look {
    ink: Rgb,
    snout: Rgb,
    nose: Rgb,
    dot: Option<Rgb>,
}

fn look(state: State, taskbar: Taskbar) -> Look {
    let (ink, quiet_snout, paused_snout) = match taskbar {
        Taskbar::Light => (NAVY, [0xE9, 0xE4, 0xDC], [0xB9, 0xAF, 0xA3]),
        Taskbar::Dark => (ICE, [0x20, 0x20, 0x20], [0x6E, 0x66, 0x5D]),
    };
    let (snout, dot) = match state {
        State::Idle => (quiet_snout, None),
        State::Listening => (COBALT, Some([0xD1, 0x45, 0x2F])),
        State::Cleaning => (COBALT, Some([0x1F, 0xA3, 0x8A])),
        State::Loading => (paused_snout, None),
        State::NeedsYou => (quiet_snout, Some([0xE0, 0xA8, 0x00])),
    };
    // The nose is the bear's colour, except on the dark idle snout, where
    // the ice bear's own colour shows it.
    Look { ink, snout, nose: ink, dot }
}

/// A shape in the 32-unit design grid.
enum Shape {
    Ellipse { cx: f32, cy: f32, rx: f32, ry: f32 },
}

impl Shape {
    fn contains(&self, x: f32, y: f32) -> bool {
        match *self {
            Shape::Ellipse { cx, cy, rx, ry } => {
                let (dx, dy) = ((x - cx) / rx, (y - cy) / ry);
                dx * dx + dy * dy <= 1.0
            }
        }
    }
}

fn circle(cx: f32, cy: f32, r: f32) -> Shape {
    Shape::Ellipse { cx, cy, rx: r, ry: r }
}

/// The icon for `state` as straight RGBA, [`SIZE`] square.
pub fn render(state: State, taskbar: Taskbar) -> Vec<u8> {
    let l = look(state, taskbar);
    // Painted in order; `None` paint clears (the gap around the dot).
    let mut layers: Vec<(Shape, Option<Rgb>)> = vec![
        (circle(10.0, 9.0, 5.0), Some(l.ink)),
        (circle(22.0, 9.0, 5.0), Some(l.ink)),
        (circle(16.0, 18.0, 12.0), Some(l.ink)),
        (Shape::Ellipse { cx: 16.0, cy: 22.0, rx: 6.2, ry: 4.8 }, Some(l.snout)),
        (Shape::Ellipse { cx: 16.0, cy: 19.4, rx: 2.7, ry: 1.8 }, Some(l.nose)),
    ];
    if let Some(dot) = l.dot {
        layers.push((circle(26.0, 26.0, 6.0), None));
        layers.push((circle(26.0, 26.0, 4.6), Some(dot)));
    }
    const SS: u32 = 4;
    let scale = 32.0 / SIZE as f32;
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = (x as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let fy = (y as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    let top = layers.iter().rev().find(|(s, _)| s.contains(fx, fy));
                    if let Some((_, Some(c))) = top {
                        acc[0] += f32::from(c[0]);
                        acc[1] += f32::from(c[1]);
                        acc[2] += f32::from(c[2]);
                        acc[3] += 1.0;
                    }
                }
            }
            let covered = acc[3];
            if covered == 0.0 {
                px.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let alpha = covered / (SS * SS) as f32;
                px.extend_from_slice(&[
                    (acc[0] / covered).round() as u8,
                    (acc[1] / covered).round() as u8,
                    (acc[2] / covered).round() as u8,
                    (alpha * 255.0).round() as u8,
                ]);
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], x: f32, y: f32) -> [u8; 4] {
        let (x, y) = ((x * SIZE as f32 / 32.0) as usize, (y * SIZE as f32 / 32.0) as usize);
        let i = (y * SIZE as usize + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn kde_colour_schemes_pick_the_glyph() {
        let dark = "[General]\nX=1\n\n[Colors:Window]\nBackgroundAlternate=1,2,3\nBackgroundNormal=32,35,38\n";
        assert_eq!(kde_taskbar(dark), Some(Taskbar::Dark));
        assert_eq!(kde_taskbar("[Colors:Window]\nBackgroundNormal=239,240,241\n"), Some(Taskbar::Light));
        assert_eq!(kde_taskbar("[General]\nColorScheme=Foo\n"), None);
    }

    #[test]
    fn the_snout_and_dot_carry_the_state() {
        let idle = render(State::Idle, Taskbar::Light);
        assert_eq!(idle.len(), (SIZE * SIZE * 4) as usize);
        assert_eq!(at(&idle, 16.0, 12.0), [0x0E, 0x1A, 0x30, 255], "head");
        assert_eq!(at(&idle, 0.5, 31.5)[3], 0, "corner is transparent");
        assert_eq!(at(&idle, 12.0, 24.0), [0xE9, 0xE4, 0xDC, 255], "idle snout");
        assert_eq!(at(&idle, 27.0, 27.0)[3], 0, "no dot when idle");

        let listening = render(State::Listening, Taskbar::Light);
        assert_eq!(at(&listening, 12.0, 24.0), [0x2F, 0x6F, 0xDB, 255], "cobalt snout");
        assert_eq!(at(&listening, 26.5, 26.5), [0xD1, 0x45, 0x2F, 255], "red dot");
        assert_eq!(at(&listening, 20.5, 26.0)[3], 0, "gap around the dot");

        let dark = render(State::Idle, Taskbar::Dark);
        assert_eq!(at(&dark, 16.0, 12.0), [0xDD, 0xE8, 0xFB, 255], "ice bear");
    }
}

/// Writes every state's icon as raw RGBA to `$BD_ICON_DUMP/<state>-<taskbar>.rgba`
/// for eyeballing: `BD_ICON_DUMP=/tmp/icons cargo test -p bd-app dump -- --ignored`.
#[cfg(test)]
#[test]
#[ignore = "writes files; for a visual check"]
fn dump() {
    let dir = std::path::PathBuf::from(std::env::var_os("BD_ICON_DUMP").expect("set BD_ICON_DUMP"));
    std::fs::create_dir_all(&dir).unwrap();
    for state in [State::Idle, State::Listening, State::Cleaning, State::Loading, State::NeedsYou] {
        for taskbar in [Taskbar::Light, Taskbar::Dark] {
            let name = format!("{state:?}-{taskbar:?}.rgba").to_lowercase();
            std::fs::write(dir.join(name), render(state, taskbar)).unwrap();
        }
    }
}
