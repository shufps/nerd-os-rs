//! The Kconfig values as Rust constants (`config/sdkconfig.rs`): from the
//! project's sdkconfig in the firmware build (RUST_CORE_SDKCONFIG, set by
//! CMakeLists.txt), from sdkconfig.defaults on the host.
//!
//! Host builds (tests) only: the firmware links these from ESP-IDF components
//! instead. Compiles libsecp256k1 with the same configuration as
//! components/libsecp256k1, and stand-ins for the ESP-IDF functions rust_core
//! calls (host/esp_host.c), whose behaviour the tests can script.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=host");
    sdkconfig();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        idf_bindings();
        return;
    }

    let secp = "../libsecp256k1/libsecp256k1";
    cc::Build::new()
        .files([
            format!("{secp}/src/secp256k1.c"),
            format!("{secp}/src/precomputed_ecmult.c"),
            format!("{secp}/src/precomputed_ecmult_gen.c"),
        ])
        .include(format!("{secp}/include"))
        .include(secp)
        .include(format!("{secp}/src"))
        // as in components/libsecp256k1/CMakeLists.txt
        .define("ECMULT_WINDOW_SIZE", "4")
        .define("ECMULT_GEN_PREC_BITS", "2")
        .define("ENABLE_MODULE_ELLSWIFT", "1")
        .define("ENABLE_MODULE_SCHNORRSIG", "1")
        .define("ENABLE_MODULE_EXTRAKEYS", "1")
        .warnings(false)
        .compile("secp256k1_host");

    cc::Build::new().file("host/esp_host.c").compile("esp_host");
}

/// `CONFIG_X=value` lines: strings as `&str`, numbers as `i64`, `y` and
/// "is not set" as `bool`.
fn sdkconfig() {
    use std::fmt::Write;
    println!("cargo:rerun-if-env-changed=RUST_CORE_SDKCONFIG");
    println!("cargo:rerun-if-env-changed=BOARD");
    // the firmware: the project's sdkconfig; the host: the defaults and the
    // values they leave to Kconfig (host/sdkconfig.host)
    let text = match std::env::var("RUST_CORE_SDKCONFIG") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::read_to_string(&path).unwrap_or_default()
        }
        Err(_) => {
            let mut t = String::new();
            for path in ["../../sdkconfig.defaults", "host/sdkconfig.host"] {
                println!("cargo:rerun-if-changed={path}");
                t += &std::fs::read_to_string(path).unwrap_or_default();
                t.push('\n');
            }
            t
        }
    };
    println!("cargo:rustc-check-cfg=cfg(coredump_flash)");
    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        let (name, value) = if let Some(rest) = line.strip_prefix("# CONFIG_") {
            match rest.strip_suffix(" is not set") {
                Some(n) => (n, "n"),
                None => continue,
            }
        } else if let Some(rest) = line.strip_prefix("CONFIG_") {
            match rest.split_once('=') {
                Some(kv) => kv,
                None => continue,
            }
        } else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }
        // the core dump API only exists with core dumps in flash
        if name == "ESP_COREDUMP_ENABLE_TO_FLASH" && value == "y" {
            println!("cargo:rustc-cfg=coredump_flash");
        }
        let rust = if value == "y" {
            "bool = true".to_string()
        } else if value == "n" {
            "bool = false".to_string()
        } else if let Some(s) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            format!("&str = \"{s}\"")
        } else if let Some(h) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            match i64::from_str_radix(h, 16) {
                Ok(v) => format!("i64 = {v}"),
                Err(_) => continue,
            }
        } else if let Ok(v) = value.parse::<i64>() {
            format!("i64 = {v}")
        } else {
            continue;
        };
        let _ = writeln!(out, "#[allow(dead_code)]\npub const {name}: {rust};");
    }
    let board = std::env::var("BOARD").unwrap_or_default();
    // cfg(board = "..."): code for one board only (the display theme links
    // only that board's images)
    println!("cargo:rustc-check-cfg=cfg(board, values(any()))");
    if !board.is_empty() {
        println!("cargo:rustc-cfg=board=\"{board}\"");
    }
    let _ = writeln!(
        out,
        "/// the board the firmware is built for (`BOARD`)\npub const BOARD: &str = \"{board}\";"
    );
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("sdkconfig.rs");
    std::fs::write(dest, out).unwrap();
}

/// The ESP-IDF bindings (`src/idf/raw.rs`), generated with bindgen from the
/// headers `layout_check.c` includes, with that file's flags from the
/// build's compile_commands.json (RUST_CORE_COMPILE_COMMANDS, set by
/// CMakeLists.txt) and the GCC's system include paths.
fn idf_bindings() {
    println!("cargo:rerun-if-env-changed=RUST_CORE_COMPILE_COMMANDS");
    println!("cargo:rerun-if-changed=layout_check.c");
    println!("cargo:rerun-if-changed=bindings.h");
    let Ok(cc_json) = std::env::var("RUST_CORE_COMPILE_COMMANDS") else {
        panic!("RUST_CORE_COMPILE_COMMANDS not set (the firmware build sets it)");
    };
    println!("cargo:rerun-if-changed={cc_json}");
    let text = std::fs::read_to_string(&cc_json).expect("compile_commands.json");
    let cmd = compile_command(&text, "layout_check.c").expect("no entry for layout_check.c");
    // JSON string, then the shell's \" in the defines
    let args: Vec<String> = json_unescape(&cmd)
        .split_whitespace()
        .map(|a| a.replace("\\\"", "\""))
        .collect();

    // -I/-isystem/-D/-include/-std/-m..., and the compiler for its system paths
    let mut clang = Vec::new();
    let mut it = args.iter().peekable();
    let compiler = it.next().cloned().unwrap_or_default();
    while let Some(a) = it.next() {
        if a == "-isystem" || a == "-include" || a == "-I" {
            if let Some(v) = it.next() {
                clang.push(a.clone());
                clang.push(v.clone());
            }
        } else if a.starts_with("-I") || a.starts_with("-D") || a.starts_with("-std=") {
            clang.push(a.clone());
        }
    }
    for dir in system_includes(&compiler) {
        clang.push("-isystem".into());
        clang.push(dir);
    }

    // the wrapper: layout_check.c's includes and bindings.h
    let src = std::fs::read_to_string("layout_check.c").unwrap()
        + &std::fs::read_to_string("bindings.h").unwrap();
    let mut wrapper = String::new();
    for l in src
        .lines()
        .filter(|l| l.trim_start().starts_with("#include"))
    {
        wrapper.push_str(l);
        wrapper.push('\n');
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("idf_wrapper.h"), wrapper).unwrap();

    let t = std::time::Instant::now();
    let b = bindgen::Builder::default()
        .header(out.join("idf_wrapper.h").to_string_lossy())
        .clang_arg("--target=xtensa-esp-elf")
        .clang_arg("-mcpu=esp32s3")
        .clang_args(&clang)
        .use_core()
        .ctypes_prefix("core::ffi")
        .layout_tests(false)
        .derive_default(true)
        .generate_comments(false)
        .generate()
        .expect("bindgen");
    b.write_to_file(out.join("idf.rs")).unwrap();
    println!("cargo:warning=idf bindings generated in {:?}", t.elapsed());
}

/// The `command` (or joined `arguments`) of the compile_commands.json entry
/// for `file` (without a JSON parser: the fields are plain strings).
fn compile_command(json: &str, file: &str) -> Option<String> {
    for entry in json.split("},") {
        if !entry.contains(&format!("{file}\"")) {
            continue;
        }
        let key = "\"command\": \"";
        let start = entry.find(key)? + key.len();
        let rest = &entry[start..];
        let mut end = 0;
        let bytes = rest.as_bytes();
        while end < bytes.len() {
            if bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\') {
                break;
            }
            end += 1;
        }
        return Some(rest[..end].to_string());
    }
    None
}

/// GCC's `#include <...>` search paths (newlib, the compiler's headers).
fn system_includes(compiler: &str) -> Vec<String> {
    let Ok(out) = std::process::Command::new(compiler)
        .args(["-xc", "-E", "-v", "/dev/null", "-o", "/dev/null"])
        .output()
    else {
        return Vec::new();
    };
    let err = String::from_utf8_lossy(&out.stderr);
    let mut dirs = Vec::new();
    let mut on = false;
    for l in err.lines() {
        if l.starts_with("#include <...> search starts here") {
            on = true;
        } else if l.starts_with("End of search list") {
            break;
        } else if on {
            dirs.push(l.trim().to_string());
        }
    }
    dirs
}

/// A JSON string's content without its escapes.
fn json_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}
