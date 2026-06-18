//! Lark/Feishu web connector backed by daemon-managed CEF sessions.

pub(crate) const CONNECTOR_SLUG_LARK: &str = "lark-browser";
pub(crate) const CONNECTOR_SLUG_FEISHU: &str = "feishu-browser";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Brand {
    Lark,
    Feishu,
}

impl Brand {
    pub(crate) fn from_slug(slug: &str) -> Option<Brand> {
        match slug {
            CONNECTOR_SLUG_LARK => Some(Brand::Lark),
            CONNECTOR_SLUG_FEISHU => Some(Brand::Feishu),
            _ => None,
        }
    }
    pub(crate) fn slug(&self) -> &'static str {
        match self {
            Brand::Lark => CONNECTOR_SLUG_LARK,
            Brand::Feishu => CONNECTOR_SLUG_FEISHU,
        }
    }
    pub(crate) fn platform(&self) -> &'static str {
        self.slug()
    }
    pub(crate) fn web_url(&self) -> &'static str {
        match self {
            Brand::Lark => "https://web.larksuite.com/",
            Brand::Feishu => "https://web.feishu.cn/",
        }
    }
}

pub(crate) async fn run_subscriber() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_from_slug_maps_both_brands() {
        assert_eq!(Brand::from_slug("lark-browser"), Some(Brand::Lark));
        assert_eq!(Brand::from_slug("feishu-browser"), Some(Brand::Feishu));
        assert_eq!(Brand::from_slug("gmail-browser"), None);
    }

    #[test]
    fn brand_web_url_and_platform() {
        assert_eq!(Brand::Lark.web_url(), "https://web.larksuite.com/");
        assert_eq!(Brand::Feishu.web_url(), "https://web.feishu.cn/");
        assert_eq!(Brand::Lark.platform(), "lark-browser");
        assert_eq!(Brand::Feishu.platform(), "feishu-browser");
    }
}
