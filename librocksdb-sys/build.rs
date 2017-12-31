extern crate cc;
extern crate bindgen;

use std::env;
use std::fs;
use std::path::PathBuf;

macro_rules! return_some {
    ( $e:expr ) => ( let v = $e; if v.is_some() { return v; } );
}

fn link(name: &str, bundled: bool) {
    use std::env::var;
    let target = var("TARGET").unwrap();
    let target: Vec<_> = target.split('-').collect();
    if target.get(2) == Some(&"windows") {
        println!("cargo:rustc-link-lib=dylib={}", name);
        if bundled && target.get(3) == Some(&"gnu") {
            let dir = var("CARGO_MANIFEST_DIR").unwrap();
            println!("cargo:rustc-link-search=native={}/{}", dir, target[0]);
        }
    }
}

fn fail_on_empty_directory(name: &str) {
    if fs::read_dir(name).unwrap().count() == 0 {
        println!(
            "The `{}` directory is empty, did you forget to pull the submodules?",
            name
        );
        println!("Try `git submodule update --init --recursive`");
        panic!();
    }
}

fn find_cache_dir(filename: &'static str) -> Option<PathBuf> {

    let lib_exists = |pbdir: PathBuf| -> Option<PathBuf> {
        let p = pbdir.join(filename);
        if fs::metadata(p).is_ok() {
            Some(pbdir)
        } else {
            None
        }
    };

    return_some!(env::var("LIBROCKSDB_DIR").map(PathBuf::from).ok().and_then(&lib_exists));
    return_some!(env::var("CARGO_HOME").map(|d| PathBuf::from(d).join("libcache")).ok().and_then(&lib_exists));
    return_some!(env::home_dir().map(|d| PathBuf::from(d).join(".cargo").join("libcache")).and_then(&lib_exists));
    None
}

fn write_cached_lib(cachedir: Option<&str>, filename: &str) -> Option<u64> {
    let find_dir = |dir: Option<&str>| {
        return_some!(dir.map(PathBuf::from));
        return_some!(env::var("LIBROCKSDB_DIR").map(PathBuf::from).ok());
        return_some!(env::var("CARGO_HOME").map(|d| PathBuf::from(d).join("libcache")).ok());
        return_some!(env::home_dir().map(|d| PathBuf::from(d).join(".cargo").join("libcache")));
        None
    };

    if let (Some(cache_dir), Some(output_dir)) = (find_dir(cachedir), env::var("OUT_DIR").map(PathBuf::from).ok()) {
       fs::create_dir_all(&cache_dir).expect(&format!("failed to create lib cache dir {:?}", cache_dir));
       fs::copy(output_dir.join(filename), cache_dir.join(filename)).ok()
    } else {
        None
    }
}

fn build_rocksdb() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rocksdb/");

    let bindings = bindgen::Builder::default()
        .header("rocksdb/include/rocksdb/c.h")
        .hide_type("max_align_t") // https://github.com/rust-lang-nursery/rust-bindgen/issues/550
        .ctypes_prefix("libc")
        .generate()
        .expect("unable to generate rocksdb bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("unable to write rocksdb bindings");

    if let (Some(cache_dir), Some(output_dir)) = (find_cache_dir("librocksdb.a"), env::var("OUT_DIR").map(PathBuf::from).ok()) {
        if fs::copy(cache_dir.join("librocksdb.a"), output_dir.join("librocksdb.a")).is_ok() {;
            println!("Copied cached librocksdb.a from {:?} to {:?}", cache_dir, output_dir);
            return;
        }
    }

    let mut config = cc::Build::new();
    config.include("rocksdb/include/");
    config.include("rocksdb/");
    config.include("rocksdb/third-party/gtest-1.7.0/fused-src/");
    config.include("snappy/");
    config.include(".");

    config.define("NDEBUG", Some("1"));
    config.define("SNAPPY", Some("1"));

    let mut lib_sources = include_str!("rocksdb_lib_sources.txt")
        .split(" ")
        .collect::<Vec<&'static str>>();

    // We have a pregenerated a version of build_version.cc in the local directory
    lib_sources = lib_sources
        .iter()
        .cloned()
        .filter(|file| *file != "util/build_version.cc")
        .collect::<Vec<&'static str>>();

    if cfg!(target_os = "macos") {
        config.define("OS_MACOSX", Some("1"));
        config.define("ROCKSDB_PLATFORM_POSIX", Some("1"));
        config.define("ROCKSDB_LIB_IO_POSIX", Some("1"));

    }
    if cfg!(target_os = "linux") {
        config.define("OS_LINUX", Some("1"));
        config.define("ROCKSDB_PLATFORM_POSIX", Some("1"));
        config.define("ROCKSDB_LIB_IO_POSIX", Some("1"));
        // COMMON_FLAGS="$COMMON_FLAGS -fno-builtin-memcmp"
    }
    if cfg!(target_os = "freebsd") {
        config.define("OS_FREEBSD", Some("1"));
        config.define("ROCKSDB_PLATFORM_POSIX", Some("1"));
        config.define("ROCKSDB_LIB_IO_POSIX", Some("1"));
    }

    if cfg!(windows) {
        link("rpcrt4", false);
        config.define("OS_WIN", Some("1"));

        // Remove POSIX-specific sources
        lib_sources = lib_sources
            .iter()
            .cloned()
            .filter(|file| match *file {
                "port/port_posix.cc" |
                "util/env_posix.cc" |
                "util/io_posix.cc" => false,
                _ => true,
            })
            .collect::<Vec<&'static str>>();

        // Add Windows-specific sources
        lib_sources.push("port/win/port_win.cc");
        lib_sources.push("port/win/env_win.cc");
        lib_sources.push("port/win/env_default.cc");
        lib_sources.push("port/win/win_logger.cc");
        lib_sources.push("port/win/io_win.cc");
    }

    if cfg!(target_env = "msvc") {
        config.flag("-EHsc");
    } else {
        config.flag("-std=c++11");
    }

    // this was breaking the build on travis due to
    // > 4mb of warnings emitted.
    config.flag("-Wno-unused-parameter");

    for file in lib_sources {
        let file = "rocksdb/".to_string() + file;
        config.file(&file);
    }

    config.file("build_version.cc");

    config.cpp(true);
    config.compile("librocksdb.a");
    write_cached_lib(None, "librocksdb.a");
}

fn build_snappy() {
    let mut config = cc::Build::new();
    config.include("snappy/");
    config.include(".");

    config.define("NDEBUG", Some("1"));

    if cfg!(target_env = "msvc") {
        config.flag("-EHsc");
    } else {
        config.flag("-std=c++11");
    }

    config.file("snappy/snappy.cc");
    config.file("snappy/snappy-sinksource.cc");
    config.file("snappy/snappy-c.cc");
    config.cpp(true);
    config.compile("libsnappy.a");
}

fn main() {
    fail_on_empty_directory("rocksdb");
    fail_on_empty_directory("snappy");
    build_rocksdb();
    build_snappy();
}
