//! The one filter of a running game, where the D3D12 layer finds it.
//!
//! `leonida run` builds the filter from the settings and the blocklist and installs it before the game starts;
//! the layer calls [`get`] when it creates resources, uploads and draws. Nothing is installed in a process that
//! is not running the game, so a missing filter means no filtering.

use std::sync::OnceLock;

use crate::filter::ContentFilter;

static FILTER: OnceLock<ContentFilter> = OnceLock::new();

/// Installs the filter for the rest of the process. A second call hands its filter back unused.
pub fn install(filter: ContentFilter) -> Result<&'static ContentFilter, Box<ContentFilter>> {
    let mut given = Some(filter);
    let installed = FILTER.get_or_init(|| given.take().unwrap_or_else(ContentFilter::disabled));
    match given {
        None => Ok(installed),
        Some(unused) => Err(Box::new(unused)),
    }
}

/// The installed filter, which may be disabled; `None` before [`install`].
pub fn get() -> Option<&'static ContentFilter> {
    FILTER.get()
}

/// The installed filter only when it blocks something.
pub fn active() -> Option<&'static ContentFilter> {
    get().filter(|filter| filter.is_active())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_filter_stays_and_a_second_is_handed_back() {
        assert!(get().is_none());
        let Ok(first) = install(ContentFilter::disabled()) else {
            panic!("the first install is accepted");
        };
        assert!(!first.is_active());
        assert!(get().is_some());
        assert!(active().is_none());
        assert!(install(ContentFilter::disabled()).is_err());
    }
}
