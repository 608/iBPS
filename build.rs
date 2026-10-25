use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=icon16.png");
    println!("cargo:rerun-if-changed=icon24.png");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Store the original PNGs in a multi-size ICO container without modifying them.
    let images = [(16u8, "icon16.png"), (24u8, "icon24.png")]
        .map(|(size, name)| (size, fs::read(name).expect("Cannot read icon PNG")));
    let mut ico = vec![0, 0, 1, 0, 2, 0];
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n") && png.len() >= 24);
        assert_eq!(&png[16..20], &u32::from(*size).to_be_bytes());
        assert_eq!(&png[20..24], &u32::from(*size).to_be_bytes());
        ico.extend_from_slice(&[*size, *size, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        ico.extend_from_slice(&png);
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon = out.join("ibps.ico");
    let resource = out.join("ibps.rc");
    let object = out.join("ibps-res.o");
    fs::write(&icon, ico).expect("Cannot write ICO");
    fs::write(
        &resource,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "/")
        ),
    )
    .expect("Cannot write icon resource");
    let status = Command::new(env::var_os("WINDRES").unwrap_or_else(|| "windres".into()))
        .arg("--input")
        .arg(&resource)
        .args(["--output-format", "coff", "--output"])
        .arg(&object)
        .status()
        .expect("Windows builds require MinGW windres on PATH (or WINDRES)");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bin=ibps={}", object.display());
}
