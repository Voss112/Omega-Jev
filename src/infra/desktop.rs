//! Desktop control for the agent: primary-screen capture, mouse and keyboard (SendInput).

use anyhow::{Result, bail};

/// Longest side of the screenshot the model sees; clicks are scaled back to real pixels.
pub const SHOT_MAX: u32 = 1366;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "" | "left" => Self::Left,
            "right" => Self::Right,
            "middle" => Self::Middle,
            other => bail!("bouton inconnu `{other}` (left, right, middle)"),
        })
    }
}

/// One key of a chord such as `ctrl+shift+s`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vk(pub u16);

/// `ctrl+s`, `alt+f4`, `enter`, `win+r`… → virtual keys in press order.
pub fn chord(spec: &str) -> Result<Vec<Vk>> {
    let keys: Vec<Vk> = spec.split('+').map(|k| vk(k.trim())).collect::<Result<_>>()?;
    if keys.is_empty() || keys.len() > 5 {
        bail!("combinaison de touches invalide `{spec}`");
    }
    Ok(keys)
}

fn vk(name: &str) -> Result<Vk> {
    let lower = name.to_ascii_lowercase();
    let code = match lower.as_str() {
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "win" | "meta" | "super" | "cmd" => 0x5B,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "esc" | "escape" => 0x1B,
        "space" => 0x20,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u16>().is_ok_and(|n| (1..=24).contains(&n)) => {
            0x6F + f[1..].parse::<u16>().unwrap_or(1)
        }
        c if c.len() == 1 && c.as_bytes()[0].is_ascii_alphanumeric() => u16::from(c.as_bytes()[0].to_ascii_uppercase()),
        _ => bail!("touche inconnue `{name}`"),
    };
    Ok(Vk(code))
}

/// Screenshot pixel → screen pixel, given the real screen size.
pub fn to_screen(x: i32, y: i32, screen: (i32, i32)) -> (i32, i32) {
    let (w, h) = screen;
    let scale = scale(w, h);
    let clamp = |v: f64, max: i32| (v.round() as i32).clamp(0, max - 1);
    (clamp(f64::from(x) / scale, w), clamp(f64::from(y) / scale, h))
}

fn scale(w: i32, h: i32) -> f64 {
    let longest = w.max(h).max(1);
    (f64::from(SHOT_MAX) / f64::from(longest)).min(1.0)
}

/// PNG of the primary screen, downscaled to `SHOT_MAX`, with the real screen size.
pub fn screenshot() -> Result<(Vec<u8>, (i32, i32))> {
    let (rgba, w, h) = platform::capture()?;
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or_else(|| anyhow::anyhow!("capture invalide"))?;
    let s = scale(w, h);
    let img = if s < 1.0 {
        let (tw, th) = ((f64::from(w) * s).round() as u32, (f64::from(h) * s).round() as u32);
        image::imageops::resize(&img, tw.max(1), th.max(1), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok((png, (w, h)))
}

pub fn screen_size() -> (i32, i32) {
    platform::screen_size()
}

pub fn click(x: i32, y: i32, button: Button, double: bool) -> Result<()> {
    platform::move_to(x, y)?;
    for _ in 0..if double { 2 } else { 1 } {
        platform::button(button)?;
    }
    Ok(())
}

pub fn move_to(x: i32, y: i32) -> Result<()> {
    platform::move_to(x, y)
}

pub fn scroll(clicks: i32) -> Result<()> {
    platform::scroll(clicks.clamp(-50, 50))
}

pub fn type_text(text: &str) -> Result<()> {
    if text.chars().count() > 5000 {
        bail!("texte trop long (5000 caractères max)");
    }
    platform::type_text(text)
}

pub fn press(keys: &[Vk]) -> Result<()> {
    platform::press(keys)
}

#[cfg(windows)]
mod platform {
    use anyhow::{Result, bail};
    use windows_sys::Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
            DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
                MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
                MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
            },
            WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos},
        },
    };

    use super::{Button, Vk};

    pub fn screen_size() -> (i32, i32) {
        // SAFETY: no pointer arguments.
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
    }

    pub fn capture() -> Result<(Vec<u8>, i32, i32)> {
        let (w, h) = screen_size();
        if w <= 0 || h <= 0 {
            bail!("écran introuvable");
        }
        let mut bgra = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: every GDI object created here is released before returning; the buffer holds w*h*4 bytes,
        // exactly what GetDIBits writes for a top-down 32-bit bitmap of that size.
        let rows = unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bitmap);
            BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let rows = GetDIBits(mem, bitmap, 0, h as u32, bgra.as_mut_ptr().cast(), &raw mut info, DIB_RGB_COLORS);
            DeleteObject(bitmap);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            rows
        };
        if rows != h {
            bail!("capture d'écran impossible (session verrouillée ?)");
        }
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok((bgra, w, h))
    }

    fn send(inputs: &[INPUT]) -> Result<()> {
        let n = u32::try_from(inputs.len())?;
        // SAFETY: a valid slice of fully initialised INPUT structures.
        let sent = unsafe { SendInput(n, inputs.as_ptr(), size_of::<INPUT>() as i32) };
        if sent != n {
            bail!("entrée refusée par Windows (fenêtre administrateur au premier plan ?)");
        }
        Ok(())
    }

    fn mouse(flags: u32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    fn key(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    pub fn move_to(x: i32, y: i32) -> Result<()> {
        // SAFETY: no pointer arguments.
        if unsafe { SetCursorPos(x, y) } == 0 {
            bail!("déplacement de la souris refusé");
        }
        Ok(())
    }

    pub fn button(button: Button) -> Result<()> {
        let (down, up) = match button {
            Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };
        send(&[mouse(down, 0), mouse(up, 0)])
    }

    pub fn scroll(clicks: i32) -> Result<()> {
        // Positive = up, one notch = 120.
        send(&[mouse(MOUSEEVENTF_WHEEL, clicks * 120)])
    }

    pub fn type_text(text: &str) -> Result<()> {
        let inputs: Vec<INPUT> = text
            .encode_utf16()
            .flat_map(|unit| [key(0, unit, KEYEVENTF_UNICODE), key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)])
            .collect();
        if inputs.is_empty() { Ok(()) } else { send(&inputs) }
    }

    pub fn press(keys: &[Vk]) -> Result<()> {
        let down = keys.iter().map(|k| key(k.0, 0, 0));
        let up = keys.iter().rev().map(|k| key(k.0, 0, KEYEVENTF_KEYUP));
        send(&down.chain(up).collect::<Vec<_>>())
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::{Result, bail};

    use super::{Button, Vk};

    pub fn screen_size() -> (i32, i32) {
        (0, 0)
    }
    pub fn capture() -> Result<(Vec<u8>, i32, i32)> {
        bail!("contrôle du bureau : Windows uniquement")
    }
    pub fn move_to(_: i32, _: i32) -> Result<()> {
        bail!("contrôle du bureau : Windows uniquement")
    }
    pub fn button(_: Button) -> Result<()> {
        bail!("contrôle du bureau : Windows uniquement")
    }
    pub fn scroll(_: i32) -> Result<()> {
        bail!("contrôle du bureau : Windows uniquement")
    }
    pub fn type_text(_: &str) -> Result<()> {
        bail!("contrôle du bureau : Windows uniquement")
    }
    pub fn press(_: &[Vk]) -> Result<()> {
        bail!("contrôle du bureau : Windows uniquement")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_to_virtual_keys() {
        assert_eq!(chord("ctrl+s").unwrap(), [Vk(0x11), Vk(b'S'.into())]);
        assert_eq!(chord("Alt+F4").unwrap(), [Vk(0x12), Vk(0x73)]);
        assert_eq!(chord("enter").unwrap(), [Vk(0x0D)]);
        assert!(chord("ctrl+nope").is_err());
        assert!(chord("f25").is_err());
    }

    #[test]
    fn screenshot_coordinates_map_back_to_the_screen() {
        // 4K screen, model sees 1366 wide: its centre is the screen's centre.
        assert_eq!(to_screen(683, 384, (3840, 2160)), (1920, 1079));
        assert_eq!(to_screen(5000, -3, (3840, 2160)), (3839, 0));
        // Small screen: no downscale.
        assert_eq!(to_screen(10, 20, (1280, 720)), (10, 20));
    }
}
