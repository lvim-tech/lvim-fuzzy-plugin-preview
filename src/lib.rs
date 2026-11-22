//! lvim-fuzzy-plugin-preview: a PROCESS plugin (JSON lines on stdin / stdout, see lvim-plugin's `procplug`) that computes the
//! previews of a directory, a CSV / TSV table, an archive and a binary file. It never draws: it answers with one
//! `preview` instruction (see `lvim_fuzzy_ui::provided`) and the tool that asked draws it. A file it has no business
//! with is answered `{"op":"pass"}`.
//!
//! The request's context: `{ "call": "preview", "provider": "dir" | "table" | "archive" | "binary", "path": "…",
//! "config": { … } }` — `config` is the plugin's settings merged from its defaults, the shared override file and the
//! asking tool's own override.

use std::io::{BufRead, Write};
use std::path::Path;

use lvim_fuzzy_ui::{archive, fsinfo, listview, media};
use serde_json::{Value, json};

/// The plugin's loop: JSON lines on stdin, answers on stdout (see the module). Returns when stdin closes or a `quit`
/// event comes.
pub fn run() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        match req["method"].as_str() {
            Some("call") => {
                let id = req["id"].clone();
                let reply = match answer(&req["params"]) {
                    Ok(ins) => json!({"id": id, "result": [ins]}),
                    Err(e) => json!({"id": id, "error": e}),
                };
                let _ = writeln!(out, "{reply}");
                let _ = out.flush();
            }
            Some("event") if req["params"]["event"] == "quit" => break,
            _ => {}
        }
    }
}

/// The answer to one call: the engine the configuration gives this file's type, run; or `pass`.
pub fn answer(ctx: &Value) -> Result<Value, String> {
    if ctx["call"] != "preview" {
        return Err(format!("no such function: {}", ctx["call"].as_str().unwrap_or("?")));
    }
    let path = Path::new(ctx["path"].as_str().ok_or("no `path`")?);
    let cfg = &ctx["config"];
    let Some((group, engine)) = lvim_fuzzy_plugin::types::engine_for(path, cfg) else { return Ok(json!({"op": "pass"})) };
    Ok(match engine.as_str() {
        "dir" => dir(path, cfg),
        "table" => table(path, cfg),
        "archive" => archive_tree(path, cfg),
        "binary" => binary(path),
        "hex" => hex(path, cfg),
        "media" => media_preview(path, ctx),
        // the tool is told how to show the file; it carries it out
        "nvim" | "builtin" => json!({"op": "preview", "renderer": engine}),
        "host" | "pass" => json!({"op": "pass"}),
        name => match cfg["engines"][name].as_object() {
            Some(e) if e.get("kind").and_then(Value::as_str).unwrap_or("command") == "command" => command(path, name, &cfg["engines"][name], cfg),
            Some(e) => json!({"op": "preview", "missing": [format!("engines.{name}: kind `{}` — only \"command\" can be defined", e.get("kind").and_then(Value::as_str).unwrap_or(""))]}),
            None => json!({
                "op": "preview",
                "missing": [format!("types.{group}: engine \"{name}\" is neither built in (dir, table, archive, binary, hex, media, nvim, builtin, host) nor defined in [engines.{name}]")],
                "renderer": "nvim",
            }),
        },
    })
}

/// A `kind = "command"` engine: `command` with `{}` = the file; its output is the preview.
fn command(path: &Path, name: &str, e: &Value, cfg: &Value) -> Value {
    let template = e["command"].as_str().unwrap_or("");
    let prog = template.split_whitespace().next().unwrap_or("");
    let max = cfg["limits"]["max_lines"].as_u64().unwrap_or(500) as usize;
    let plain = || -> Vec<String> {
        std::fs::read_to_string(path).map(|s| s.lines().take(max).map(str::to_string).collect()).unwrap_or_default()
    };
    if prog.is_empty() || !lvim_fuzzy_plugin::have(prog) {
        return json!({
            "op": "preview",
            "missing": [format!("`{}` is not installed (engine `{name}`)", if prog.is_empty() { "?" } else { prog })],
            "body": {"type": "text", "ansi": false, "lines": plain()},
        });
    }
    let line = lvim_fuzzy_plugin::types::expand(template, path);
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&line)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let lines: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().take(max).map(str::to_string).collect();
            json!({"op": "preview", "body": {"type": "text", "ansi": e["ansi"].as_bool().unwrap_or(false), "lines": lines}})
        }
        _ => json!({
            "op": "preview",
            "missing": [format!("engine `{name}` failed: {line}")],
            "body": {"type": "text", "ansi": false, "lines": plain()},
        }),
    }
}

fn dir(path: &Path, cfg: &Value) -> Value {
    if !path.is_dir() {
        return json!({"op": "pass"});
    }
    let hidden = cfg["dir"]["hidden"].as_bool().unwrap_or(false);
    let entries = fsinfo::list_dir(path, hidden, fsinfo::Sort::Name, false).unwrap_or_default();
    let rows: Vec<Value> = entries
        .iter()
        .filter(|e| !e.is_parent())
        .map(|e| {
            json!({
                "name": e.name,
                "dir": e.is_dir(),
                "link": matches!(e.kind, fsinfo::Kind::Link | fsinfo::Kind::LinkDir),
                "broken": e.broken,
                "exec": e.is_exec(),
                "size": e.size,
                "mtime": e.mtime,
            })
        })
        .collect();
    json!({"op": "preview", "body": {"type": "entries", "rows": rows}})
}

fn table(path: &Path, cfg: &Value) -> Value {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !path.is_file() || !(name.ends_with(".csv") || name.ends_with(".tsv")) {
        return json!({"op": "pass"});
    }
    let limit = cfg["table"]["rows"].as_u64().unwrap_or(500) as usize;
    let mut rows = listview::read_table(path, name.ends_with(".tsv"));
    rows.truncate(limit);
    json!({"op": "preview", "body": {"type": "table", "rows": rows}})
}

fn archive_tree(path: &Path, cfg: &Value) -> Value {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !path.is_file() || archive::format(&name).is_none() {
        return json!({"op": "pass"});
    }
    match archive::tree(path) {
        Ok(mut rows) => {
            rows.truncate(cfg["archive"]["max_rows"].as_u64().unwrap_or(5000) as usize);
            let rows: Vec<Value> =
                rows.iter().map(|r| json!({"depth": r.depth, "name": r.name, "dir": r.dir, "size": r.size})).collect();
            json!({"op": "preview", "body": {"type": "tree", "rows": rows}})
        }
        // a program that is not installed is said as that; any other failure is the text of the error
        Err(e) if e.contains("is needed") => json!({"op": "preview", "missing": [e]}),
        Err(e) => json!({"op": "preview", "body": {"type": "text", "lines": [e]}}),
    }
}

/// A picture, a video (a frame), a PDF page, a font, a sound's cover, an office document: the facts about the file, what
/// this machine lacks for a full preview, and the picture as a PNG in the cache. The host says how big a picture it can
/// show (`max_px`) and which position of a multi-page file it wants (`skip`).
fn media_preview(path: &Path, ctx: &Value) -> Value {
    let px = |i: usize, d: u64| ctx["max_px"].get(i).and_then(Value::as_u64).unwrap_or(d).clamp(16, 8192) as u32;
    let skip = ctx["skip"].as_u64().unwrap_or(0) as u32;
    let Some(j) = media::describe_json(path, (px(0, 1000), px(1, 800)), skip) else { return json!({"op": "pass"}) };
    json!({
        "op": "preview",
        "facts": j["info"],
        "missing": j["missing"],
        "positions": j["positions"],
        "picture": j["picture"],
    })
}

/// A hex dump of the start of a file: the offset, 16 bytes in hex, the same as characters; the facts above it.
/// Coloured (ANSI): NUL dim, printable green, white space yellow, the rest cyan.
fn hex(path: &Path, cfg: &Value) -> Value {
    use std::io::Read;
    if !path.is_file() {
        return json!({"op": "pass"});
    }
    let limit = cfg["hex"]["bytes"].as_u64().unwrap_or(8192).clamp(16, 1 << 20) as usize;
    let mut buf = Vec::new();
    match std::fs::File::open(path).and_then(|f| f.take(limit as u64).read_to_end(&mut buf)) {
        Ok(_) => {}
        Err(e) => return json!({"op": "preview", "body": {"type": "text", "lines": [e.to_string()]}}),
    }
    let colour = |b: u8| match b {
        0 => "\x1b[90m",
        0x21..=0x7e => "\x1b[32m",
        b' ' | b'\t' | b'\n' | b'\r' => "\x1b[33m",
        _ => "\x1b[36m",
    };
    let lines: Vec<String> = buf
        .chunks(16)
        .enumerate()
        .map(|(i, row)| {
            let mut l = format!("\x1b[90m{:08x}\x1b[0m  ", i * 16);
            for k in 0..16 {
                match row.get(k) {
                    Some(&b) => l += &format!("{}{b:02x}\x1b[0m ", colour(b)),
                    None => l += "   ",
                }
                if k == 7 {
                    l.push(' ');
                }
            }
            l.push_str(" \x1b[90m│\x1b[0m");
            for &b in row {
                let c = if (0x20..0x7f).contains(&b) { b as char } else { '.' };
                l += &format!("{}{c}\x1b[0m", colour(b));
            }
            l.push_str("\x1b[90m│\x1b[0m");
            l
        })
        .collect();
    let facts: Vec<Value> = if listview::is_binary(path) {
        listview::binary_note(path).into_iter().map(|(k, v)| json!([k, v])).collect()
    } else {
        Vec::new()
    };
    json!({"op": "preview", "facts": facts, "body": {"type": "text", "ansi": true, "lines": lines}})
}

fn binary(path: &Path) -> Value {
    if !listview::is_binary(path) {
        return json!({"op": "pass"});
    }
    let facts: Vec<Value> = listview::binary_note(path).into_iter().map(|(k, v)| json!([k, v])).collect();
    json!({"op": "preview", "facts": facts})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lvim-fuzzy-plugin-preview-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The plugin's own default configuration, as a host would send it (plugin.toml's [config], merged by `host`).
    fn defaults() -> Value {
        let m: toml::Table = toml::from_str(include_str!("../plugin.toml")).unwrap();
        serde_json::to_value(&m["config"]).unwrap()
    }

    fn with(over: Value) -> Value {
        let mut c = defaults();
        fn merge(a: &mut Value, b: &Value) {
            match (a, b) {
                (Value::Object(x), Value::Object(y)) => y.iter().for_each(|(k, v)| merge(x.entry(k.clone()).or_insert(Value::Null), v)),
                (a, b) => *a = b.clone(),
            }
        }
        merge(&mut c, &over);
        c
    }

    fn ask(path: &Path, cfg: Value) -> Value {
        let cfg = with(cfg);
        answer(&json!({"call": "preview", "path": path, "config": cfg})).unwrap()
    }

    #[test]
    fn a_directory_is_entries_and_hidden_follows_the_config() {
        let d = tmp("dir");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        std::fs::write(d.join(".hid"), "x").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let rows = |v: &Value| v["body"]["rows"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(rows(&ask(&d, json!({"dir": {"hidden": false}}))), vec!["sub", "a.txt"]);
        assert_eq!(rows(&ask(&d, json!({"dir": {"hidden": true}}))), vec!["sub", ".hid", "a.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_table_is_cut_at_the_configured_rows() {
        let d = tmp("table");
        let f = d.join("t.csv");
        std::fs::write(&f, "a,b\n1,2\n3,4\n5,6\n").unwrap();
        let v = ask(&f, json!({"table": {"rows": 3}}));
        assert_eq!(v["body"]["type"], "table");
        assert_eq!(v["body"]["rows"].as_array().unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_binary_file_is_a_note_and_an_archive_is_not_a_binary() {
        let d = tmp("bin");
        std::fs::write(d.join("b.bin"), [0u8, 1, 2]).unwrap();
        let v = ask(&d.join("b.bin"), json!({}));
        assert_eq!(v["facts"][0][1], "binary file");
        // a .zip is binary too, but the group with an extension is the more specific
        std::fs::write(d.join("a.zip"), [0u8, 1, 2]).unwrap();
        let v = ask(&d.join("a.zip"), json!({}));
        assert!(v.get("facts").is_none(), "{v}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hex_dumps_a_binary_file() {
        let d = tmp("hex");
        std::fs::write(d.join("b.bin"), (0u8..40).collect::<Vec<u8>>()).unwrap();
        let v = ask(&d.join("b.bin"), json!({"types": {"binary": {"kind": ["binary"], "engine": "hex"}}}));
        assert_eq!(v["facts"][0][1], "binary file");
        let lines = v["body"]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].as_str().unwrap().contains("00000000") && lines[2].as_str().unwrap().contains("00000020"));
        // the number of bytes is configurable
        let v = ask(&d.join("b.bin"), json!({"types": {"binary": {"kind": ["binary"], "engine": "hex"}}, "hex": {"bytes": 16}}));
        assert_eq!(v["body"]["lines"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_type_decides_the_engine_by_ext_name_kind_or_mime() {
        let d = tmp("types");
        let (rs, png, mk) = (d.join("a.rs"), d.join("p.png"), d.join("Makefile"));
        std::fs::write(&rs, "fn main() {}").unwrap();
        std::fs::write(&png, [0x89, b'P', 0, 1]).unwrap();
        std::fs::write(&mk, "all:").unwrap();
        // text -> Neovim by default
        assert_eq!(ask(&rs, json!({}))["renderer"], "nvim");
        // one extension to another engine
        assert_eq!(ask(&rs, json!({"types": {"rust": {"ext": ["rs"], "engine": "builtin"}}}))["renderer"], "builtin");
        // a file name
        assert_eq!(ask(&mk, json!({"types": {"make": {"name": ["makefile"], "engine": "builtin"}}}))["renderer"], "builtin");
        // a MIME pattern: image/* for the picture; the text file is not an image
        let img = json!({"types": {"images": {"mime": ["image/*"], "engine": "host"}}});
        assert_eq!(ask(&png, img.clone())["op"], "pass");
        assert_eq!(ask(&rs, img)["renderer"], "nvim");
        // a priority between two groups that both match
        let two = json!({"types": {"a": {"ext": ["rs"], "engine": "builtin"}, "b": {"ext": ["rs"], "engine": "nvim", "priority": 5}}});
        assert_eq!(ask(&rs, two)["renderer"], "nvim");
        // an engine nobody defined is said, and Neovim shows the file
        let v = ask(&rs, json!({"types": {"text": {"engine": "nosuch"}}}));
        assert!(v["missing"][0].as_str().unwrap().contains("nosuch"));
        assert_eq!(v["renderer"], "nvim");
        // a path that is not there is not mine
        assert_eq!(ask(&d.join("none.rs"), json!({}))["op"], "pass");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_command_engine_runs_its_command_and_says_when_the_program_is_missing() {
        let d = tmp("cmd");
        let f = d.join("a.rs");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        let by = |cmd: &str| json!({"types": {"text": {"engine": "x"}}, "engines": {"x": {"kind": "command", "command": cmd, "ansi": true}}});
        // a command whose output is coloured
        let v = ask(&f, by("printf '\\033[31m%s\\033[0m\\n' {}"));
        assert_eq!(v["body"]["ansi"], true);
        assert!(v["body"]["lines"][0].as_str().unwrap().contains("a.rs"));
        // the file name with a quote in it is quoted
        let q = d.join("it's.rs");
        std::fs::write(&q, "x").unwrap();
        assert!(ask(&q, by("echo {}"))["body"]["lines"][0].as_str().unwrap().contains("it's.rs"));
        // a program that is not there: said, the file is shown plain
        let v = ask(&f, by("no-such-program-here {}"));
        assert!(v["missing"][0].as_str().unwrap().contains("no-such-program-here"));
        assert_eq!(v["body"]["lines"][0], "one");
        assert_eq!(v["body"]["ansi"], false);
        // a command that fails
        assert!(ask(&f, by("false {}"))["missing"][0].as_str().unwrap().contains("failed"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_picture_is_facts_and_a_png_in_the_cache() {
        let d = tmp("media");
        // a 3 x 2 PNG made here
        let f = d.join("p.png");
        let mut img = image::RgbaImage::new(3, 2);
        img.put_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        img.save(&f).unwrap();
        let v = ask(&f, json!({}));
        assert_eq!(v["op"], "preview");
        assert!(v["facts"].as_array().unwrap().iter().any(|r| r[0] == "dimensions"), "{v}");
        assert_eq!((v["picture"]["width"].as_u64(), v["picture"]["height"].as_u64()), (Some(3), Some(2)));
        assert!(Path::new(v["picture"]["path"].as_str().unwrap()).is_file());
        assert_eq!(v["positions"], 1);
        // a file that is not media under the media engine: not mine
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let cfg = json!({"types": {"text": {"engine": "media"}}});
        assert_eq!(ask(&d.join("a.txt"), cfg)["op"], "pass");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_call_and_missing_path() {
        assert!(answer(&json!({"call": "other", "path": "/"})).is_err());
        assert!(answer(&json!({"call": "preview"})).is_err());
    }
}

/// The engines that need no definition (for `--list`).
pub const BUILTIN_ENGINES: [(&str, Option<&str>); 9] =
    [("dir", None), ("table", None), ("archive", None), ("binary", None), ("hex", None), ("media", None), ("nvim", None), ("builtin", None), ("host", None)];

// ----- shipping inside the tools -----

/// The argument a tool is started with to BE this plugin: `<tool> --lvim-plugin preview`.
pub const PLUGIN_ARGS: [&str; 2] = ["--lvim-plugin", "preview"];

/// The plugin's `plugin.toml` as shipped.
pub const MANIFEST: &str = include_str!("../plugin.toml");

/// For a tool's `main`: if it was started to be this plugin (`--lvim-plugin preview`), be it (never returns).
pub fn run_if_asked(args: &[String]) {
    if args.first().map(String::as_str) == Some(PLUGIN_ARGS[0]) && args.get(1).map(String::as_str) == Some(PLUGIN_ARGS[1]) {
        run();
        std::process::exit(0);
    }
}

/// Make sure the plugin is installed in the shared plugins directory `root` for the running tool: the first of lfz
/// / lvc (the Neovim picker goes through lfz) to start writes it, whoever comes first. Silent: a directory that
/// cannot be written just means no preview plugin (the tool then shows no preview of those files).
pub fn ensure_installed(root: &Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    let b = lvim_fuzzy_plugin::bundled::Bundled { name: "lvim-fuzzy-plugin-preview", version: env!("CARGO_PKG_VERSION"), manifest: MANIFEST };
    let _ = lvim_fuzzy_plugin::bundled::ensure(root, &b, &exe, &PLUGIN_ARGS.join(" "));
    // this plugin was `lvim-fuzzy-preview` before it joined the `lvim-fuzzy-plugin-*` family: the copy an older tool
    // wrote would answer next to this one
    let _ = lvim_fuzzy_plugin::bundled::retire(root, "lvim-fuzzy-preview");
}
