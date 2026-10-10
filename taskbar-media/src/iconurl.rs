//! Finding a site's own icon in its home page: `<link rel="apple-touch-icon">` first (usually a
//! 180 px PNG), then `<link rel="icon">` by size, then the usual fixed places. SVG is skipped
//! (it can't be drawn here).

/// The value of attribute `name` in one HTML tag (`<link ...>`), if any.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        // a whole attribute name: preceded by a space, followed by =
        if !lower[..at].ends_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(value.replace("&amp;", "&"));
    }
    None
}

/// `href` made absolute against the page `https://{host}/`.
fn absolute(href: &str, host: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if href.starts_with('/') {
        format!("https://{host}{href}")
    } else {
        format!("https://{host}/{href}")
    }
}

/// The icon addresses to try for a site, best first.
pub fn candidates(html: &str, host: &str) -> Vec<String> {
    let mut found: Vec<(i32, String)> = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<link") {
        let start = from + i;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e + 1);
        let tag = &html[start..end];
        from = end;
        let (Some(rel), Some(href)) = (attr(tag, "rel"), attr(tag, "href")) else { continue };
        let rel = rel.to_ascii_lowercase();
        if href.to_ascii_lowercase().split('?').next().is_some_and(|p| p.ends_with(".svg")) || attr(tag, "type").is_some_and(|t| t.contains("svg")) {
            continue;
        }
        // the largest size given ("180x180", "16x16 32x32"), 0 if none
        let size = attr(tag, "sizes")
            .map(|s| s.split_whitespace().filter_map(|d| d.split('x').next()?.parse::<i32>().ok()).max().unwrap_or(0))
            .unwrap_or(0);
        let score = if rel.contains("apple-touch-icon") {
            10_000 + size.max(180)
        } else if rel.split_whitespace().any(|r| r == "icon") {
            size
        } else {
            continue;
        };
        found.push((score, absolute(&href, host)));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<String> = found.into_iter().map(|(_, u)| u).collect();
    for fixed in ["/apple-touch-icon.png", "/favicon.ico"] {
        let u = format!("https://{host}{fixed}");
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_icon_first_and_no_svg() {
        let html = r#"<html><head>
            <link rel="icon" href="/favicon-16.png" sizes="16x16">
            <link rel="icon" type="image/svg+xml" href="/logo.svg">
            <LINK REL="icon" HREF="https://cdn.example.com/icon-192.png" SIZES="192x192">
            <link href="/apple-touch-icon-180.png" rel="apple-touch-icon">
            <link rel="stylesheet" href="/site.css">
            <link rel="shortcut icon" href="//static.example.com/favicon.ico?v=2&amp;x=1">
        </head></html>"#;
        let c = candidates(html, "www.example.com");
        assert_eq!(c[0], "https://www.example.com/apple-touch-icon-180.png");
        assert_eq!(c[1], "https://cdn.example.com/icon-192.png");
        assert_eq!(c[2], "https://www.example.com/favicon-16.png");
        assert!(c.contains(&"https://static.example.com/favicon.ico?v=2&x=1".to_string()));
        assert!(!c.iter().any(|u| u.ends_with(".svg") || u.ends_with(".css")));
        // the usual fixed places come last
        assert_eq!(c[c.len() - 2], "https://www.example.com/apple-touch-icon.png");
        assert_eq!(c[c.len() - 1], "https://www.example.com/favicon.ico");
    }

    #[test]
    fn a_page_without_links_still_has_the_fixed_places() {
        assert_eq!(candidates("<html></html>", "kick.com"), vec!["https://kick.com/apple-touch-icon.png", "https://kick.com/favicon.ico"]);
    }
}
