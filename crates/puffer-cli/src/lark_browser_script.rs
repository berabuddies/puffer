//! JS snippets for the Lark/Feishu browser connector.

/// Returns logged-in status for the Lark/Feishu web app. Logged in once the
/// messenger shell is present and we're no longer on the accounts/login page.
/// Uses only stable hooks (no hashed classes).
pub(crate) const LARK_LOGIN_MARKER_JS: &str = r#"(() => {
  const onLogin = /accounts\.(larksuite|feishu)\.(com|cn)\/.*login/i.test(location.href);
  const shell = !!document.querySelector('.lark_feedMainList, .a11y_feed_main_list, [class*="page-content-messenger"]');
  return JSON.stringify({ loggedIn: shell && !onLogin, href: location.href });
})()"#;
