//! `ratchet open <page|id>`: show a page or an optimization in the Mac app.
//!
//! The app registers the `ratchet://` scheme (`Info.plist`) and handles `ratchet://open/<page or id>` with the same
//! parser a notification click uses, so the CLI only builds the link and hands it to the system's `open`. It works with
//! the daemon down (the app starts the daemon itself), and `--print` shows the link without opening anything.

use serde_json::json;

use super::{emit_json, urlencode};
use crate::cli::OpenArgs;
use crate::io::Io;

/// The link that opens `target` (a page name or an optimization id). `None` for an empty target.
pub fn link_for(target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    Some(format!("ratchet://open/{}", urlencode(target)))
}

pub fn run_open(args: &OpenArgs, json: bool, io: &dyn Io) -> i32 {
    let Some(url) = link_for(&args.target) else {
        io.err("ratchet: open needs a page or an optimization id (see `ratchet list` for ids)\n");
        return 2;
    };
    if args.print {
        report(io, json, &url, false);
        return 0;
    }
    match launch(&url) {
        Ok(()) => {
            report(io, json, &url, true);
            0
        }
        Err(why) => {
            fail(io, json, &url, &why);
            1
        }
    }
}

fn report(io: &dyn Io, json: bool, url: &str, opened: bool) {
    if json {
        emit_json(io, &json!({"url": url, "opened": opened}));
    } else if opened {
        io.out(&format!("opened {url}\n"));
    } else {
        io.out(&format!("{url}\n"));
    }
}

fn fail(io: &dyn Io, json: bool, url: &str, why: &str) {
    if json {
        let body =
            json!({"error": {"code": "unavailable", "message": why, "fields": []}, "url": url});
        io.err(&(body.to_string() + "\n"));
    } else {
        io.err(&format!("ratchet: {why}\n"));
    }
}

#[cfg(target_os = "macos")]
fn launch(url: &str) -> Result<(), String> {
    let out = std::process::Command::new("/usr/bin/open")
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run /usr/bin/open: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "nothing on this Mac handles {url}: install Ratchet.app (and open it once) so it registers the ratchet:// scheme"
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn launch(_url: &str) -> Result<(), String> {
    Err("`ratchet open` shows things in the Mac app; use --print for the link".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_names_a_page_or_an_id() {
        assert_eq!(
            link_for("connectors").as_deref(),
            Some("ratchet://open/connectors")
        );
        assert_eq!(
            link_for(" opt-3f9a ").as_deref(),
            Some("ratchet://open/opt-3f9a")
        );
        assert_eq!(
            link_for("a b/c").as_deref(),
            Some("ratchet://open/a%20b%2Fc")
        );
        assert_eq!(link_for("  "), None);
    }
}
