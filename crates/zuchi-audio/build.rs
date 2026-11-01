//! macOS では、PC の音声を取得する Swift の補助プログラムをビルドする。

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let native = manifest.join("../../native/macos");
    let source = native.join("AudioTap.swift");
    let plist = native.join("Info.plist");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", plist.display());

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("対応していないアーキテクチャです: {other:?}"),
    };
    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("zuchi-audio-tap");

    let result = Command::new("swiftc")
        .args(["-O", "-target", &format!("{arch}-apple-macos14.2")])
        // CLI には bundle がないので、Info.plist をバイナリに埋め込む
        .args(["-Xlinker", "-sectcreate", "-Xlinker", "__TEXT"])
        .args(["-Xlinker", "__info_plist", "-Xlinker"])
        .arg(&plist)
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .expect("swiftc を起動できません。Xcode Command Line Tools が必要です");
    assert!(result.success(), "補助プログラムのビルドに失敗しました");

    println!("cargo:rustc-env=ZUCHI_AUDIO_TAP_PATH={}", output.display());
}
