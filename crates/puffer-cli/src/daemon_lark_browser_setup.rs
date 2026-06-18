//! Daemon-owned connect/login flow for the Lark/Feishu browser connector.

use crate::lark_browser::Brand;

pub(crate) fn connect_args_are_lark_browser(connect_args: &str) -> bool {
    brand_from_connect_args(connect_args).is_some()
}

pub(crate) fn brand_from_connect_args(connect_args: &str) -> Option<Brand> {
    connect_args.split_whitespace().next().and_then(Brand::from_slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lark_browser::Brand;

    #[test]
    fn matches_both_slugs_only() {
        assert!(connect_args_are_lark_browser("lark-browser"));
        assert!(connect_args_are_lark_browser("feishu-browser work"));
        assert!(!connect_args_are_lark_browser("gmail-browser"));
        assert!(!connect_args_are_lark_browser(""));
    }

    #[test]
    fn infers_brand_from_first_token() {
        assert_eq!(brand_from_connect_args("lark-browser foo"), Some(Brand::Lark));
        assert_eq!(brand_from_connect_args("feishu-browser"), Some(Brand::Feishu));
        assert_eq!(brand_from_connect_args("nope"), None);
    }
}
