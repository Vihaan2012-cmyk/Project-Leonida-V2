//! The language the game is told it runs in.
//!
//! The executable asks Windows itself (`GetUserDefaultLocaleName`, `GetUserDefaultGeoName`, at RVAs
//! 0x2217550 and 0x2212f80) and the runtime (`XPackageGetUserLocale`), so all three answer from one setting,
//! the `language` of `leonida.toml`. `LEONIDA_LOCALE` still overrides it.

use std::sync::Mutex;

static LANGUAGE: Mutex<Option<String>> = Mutex::new(None);

/// Sets the language code of `leonida.toml` (`en`, `pt-BR`, ...) the locale answers follow.
pub fn set_language(code: &str) {
    if let Ok(mut language) = LANGUAGE.lock() {
        *language = Some(code.trim().to_string()).filter(|c| !c.is_empty());
    }
}

/// A language code as a Windows locale name: `en` is `en-US`, `ja` is `ja-JP`, `pt-BR` stays.
pub(crate) fn tag_of(code: &str) -> String {
    if code.contains('-') {
        return code.to_string();
    }
    let region = match code.to_ascii_lowercase().as_str() {
        "en" => "US",
        "ja" => "JP",
        "ko" => "KR",
        "zh" => "CN",
        "pt" => "BR",
        "es" => "ES",
        "sv" => "SE",
        "da" => "DK",
        "cs" => "CZ",
        "el" => "GR",
        "uk" => "UA",
        "ar" => "SA",
        "he" => "IL",
        "hi" => "IN",
        other => return format!("{other}-{}", other.to_ascii_uppercase()),
    };
    format!("{}-{region}", code.to_ascii_lowercase())
}

/// The locale name the game sees: `LEONIDA_LOCALE`, else the setting, else `en-US`.
pub(crate) fn locale_tag() -> String {
    if let Some(forced) = std::env::var("LEONIDA_LOCALE")
        .ok()
        .filter(|v| !v.is_empty())
    {
        return forced;
    }
    let language = LANGUAGE.lock().ok().and_then(|l| l.clone());
    language.map_or_else(|| "en-US".to_string(), |code| tag_of(&code))
}

/// The two-letter region of a locale name, for the geographical name.
pub(crate) fn region_of(tag: &str) -> String {
    tag.split_once('-')
        .map(|(_, region)| region)
        .filter(|r| r.len() == 2)
        .unwrap_or("US")
        .to_ascii_uppercase()
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, SetLastError};

    use super::{locale_tag, region_of};
    use crate::Shim;

    /// Copies `text` and its NUL as UTF-16. Returns the length with the NUL, or 0 when `capacity` is too small
    /// (and sets `ERROR_INSUFFICIENT_BUFFER`).
    unsafe fn write_wide(buffer: *mut u16, capacity: i32, text: &str) -> i32 {
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        if buffer.is_null() || capacity < wide.len() as i32 {
            // SAFETY: sets the calling thread's last error.
            unsafe { SetLastError(ERROR_INSUFFICIENT_BUFFER) };
            return 0;
        }
        // SAFETY: `capacity` UTF-16 units are writable and the text fits.
        unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), buffer, wide.len()) };
        wide.len() as i32
    }

    pub(super) unsafe extern "win64" fn get_user_default_locale_name(
        buffer: *mut u16,
        capacity: i32,
    ) -> i32 {
        // SAFETY: the program passes a writable buffer of `capacity` units.
        unsafe { write_wide(buffer, capacity, &locale_tag()) }
    }

    pub(super) unsafe extern "win64" fn get_user_default_geo_name(
        buffer: *mut u16,
        capacity: i32,
    ) -> i32 {
        // SAFETY: the program passes a writable buffer of `capacity` units.
        unsafe { write_wide(buffer, capacity, &region_of(&locale_tag())) }
    }

    const DLLS: &[&str] = &[
        "kernel32.dll",
        "kernelbase.dll",
        "api-ms-win-core-localization-l1-2-0.dll",
        "api-ms-win-core-localization-l1-2-1.dll",
        "api-ms-win-core-localization-l1-2-2.dll",
        "api-ms-win-core-localization-l1-2-3.dll",
    ];

    pub fn shims() -> Vec<Shim> {
        let mut shims = Vec::new();
        for dll in DLLS {
            shims.push(Shim {
                dll,
                name: "GetUserDefaultLocaleName",
                address: get_user_default_locale_name as *const () as usize,
            });
            shims.push(Shim {
                dll,
                name: "GetUserDefaultGeoName",
                address: get_user_default_geo_name as *const () as usize,
            });
        }
        shims
    }
}

#[cfg(windows)]
pub(super) use windows::shims;

#[cfg(all(test, windows))]
mod tests {
    use super::windows::{get_user_default_geo_name, get_user_default_locale_name};
    use super::*;

    fn text(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|&c| c == 0).unwrap();
        String::from_utf16_lossy(&buffer[..end])
    }

    #[test]
    fn language_codes_become_locale_names() {
        assert_eq!(tag_of("en"), "en-US");
        assert_eq!(tag_of("ja"), "ja-JP");
        assert_eq!(tag_of("fr"), "fr-FR");
        assert_eq!(tag_of("pt-BR"), "pt-BR");
        assert_eq!(region_of("pt-BR"), "BR");
        assert_eq!(region_of("zh"), "US");
        assert_eq!(region_of("en-US"), "US");
    }

    #[test]
    fn the_setting_reaches_every_locale_answer() {
        // SAFETY: tests run this one test that touches the variable; no other thread reads it meanwhile.
        unsafe { std::env::remove_var("LEONIDA_LOCALE") };
        set_language("de");
        let mut name = [0u16; 85];
        // SAFETY: the buffer holds 85 units.
        let n = unsafe { get_user_default_locale_name(name.as_mut_ptr(), 85) };
        assert_eq!((n, text(&name).as_str()), (6, "de-DE"));
        let mut geo = [0u16; 85];
        // SAFETY: the buffer holds 85 units.
        let n = unsafe { get_user_default_geo_name(geo.as_mut_ptr(), 85) };
        assert_eq!((n, text(&geo).as_str()), (3, "DE"));
        let mut small = [0u16; 3];
        // SAFETY: the buffer holds 3 units.
        assert_eq!(
            unsafe { get_user_default_locale_name(small.as_mut_ptr(), 3) },
            0
        );
        set_language("");
        assert_eq!(locale_tag(), "en-US");
    }
}
