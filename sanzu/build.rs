extern crate winres;

fn main() {
    // only run if target os is windows
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() != "windows" {
        return;
    }

    if let Ok(ffmpeg_lib_dir) = std::env::var("FFMPEG_LIB_PATH") {
        println!("cargo:rustc-link-search={}", ffmpeg_lib_dir);
    }

    // only build the resource for release builds
    // as calling rc.exe might be slow
    if std::env::var("PROFILE").unwrap() == "release" {
        let mut res = winres::WindowsResource::new();
        if cfg!(unix) {
            // paths for X64 on archlinux
            res.set_toolkit_path("/usr/bin");
            // ar tool for mingw in toolkit path
            res.set_ar_path("/usr/bin/x86_64-w64-mingw32-ar");
            // windres tool
            res.set_windres_path("/usr/bin/x86_64-w64-mingw32-windres");
        }

        res.set_icon("data/icons/sanzu.ico")
            // can't use winapi crate constants for cross compiling
            // MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US )
            .set_language(0x0409)
            .set_manifest_file("data/winres/manifest.xml");
        if let Err(err) = res.compile() {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
