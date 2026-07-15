//! Build LibRaw as a static library and generate FFI bindings.
//!
//! Upstream `rsraw-sys` panics on MSVC and does not define `LIBRAW_NODLL`, which
//! breaks Windows static builds (headers default to `__declspec(dllimport)`).
//! This vendored build script supports MSVC and Clang on Windows.

use std::{env, fs, path::Path};

fn main() {
    let dir = env::var_os("OUT_DIR").unwrap();
    bindings(&dir);
    build(&dir);
}

fn build(out_dir: impl AsRef<Path>) {
    let mut libraw = cc::Build::new();

    libraw.cpp(true);
    libraw.std("c++17");
    libraw.include("LibRaw/");

    // Static link: do not use dllimport/dllexport on Windows.
    libraw.define("LIBRAW_NODLL", None);
    // Quiet some optional integration stubs
    libraw.define("LIBRAW_NO_OPENMP", None);

    libraw.file("LibRaw/src/decoders/canon_600.cpp");
    libraw.file("LibRaw/src/decoders/crx.cpp");
    libraw.file("LibRaw/src/decoders/decoders_dcraw.cpp");
    libraw.file("LibRaw/src/decoders/decoders_libraw.cpp");
    libraw.file("LibRaw/src/decoders/decoders_libraw_dcrdefs.cpp");
    libraw.file("LibRaw/src/decoders/dng.cpp");
    libraw.file("LibRaw/src/decoders/fp_dng.cpp");
    libraw.file("LibRaw/src/decoders/fuji_compressed.cpp");
    libraw.file("LibRaw/src/decoders/generic.cpp");
    libraw.file("LibRaw/src/decoders/kodak_decoders.cpp");
    libraw.file("LibRaw/src/decoders/load_mfbacks.cpp");
    libraw.file("LibRaw/src/decoders/smal.cpp");
    libraw.file("LibRaw/src/decoders/unpack.cpp");
    libraw.file("LibRaw/src/decoders/unpack_thumb.cpp");
    libraw.file("LibRaw/src/demosaic/aahd_demosaic.cpp");
    libraw.file("LibRaw/src/demosaic/ahd_demosaic.cpp");
    libraw.file("LibRaw/src/demosaic/dcb_demosaic.cpp");
    libraw.file("LibRaw/src/demosaic/dht_demosaic.cpp");
    libraw.file("LibRaw/src/demosaic/misc_demosaic.cpp");
    libraw.file("LibRaw/src/demosaic/xtrans_demosaic.cpp");
    libraw.file("LibRaw/src/integration/dngsdk_glue.cpp");
    libraw.file("LibRaw/src/integration/rawspeed_glue.cpp");
    libraw.file("LibRaw/src/metadata/adobepano.cpp");
    libraw.file("LibRaw/src/metadata/canon.cpp");
    libraw.file("LibRaw/src/metadata/ciff.cpp");
    libraw.file("LibRaw/src/metadata/cr3_parser.cpp");
    libraw.file("LibRaw/src/metadata/epson.cpp");
    libraw.file("LibRaw/src/metadata/exif_gps.cpp");
    libraw.file("LibRaw/src/metadata/fuji.cpp");
    libraw.file("LibRaw/src/metadata/hasselblad_model.cpp");
    libraw.file("LibRaw/src/metadata/identify.cpp");
    libraw.file("LibRaw/src/metadata/identify_tools.cpp");
    libraw.file("LibRaw/src/metadata/kodak.cpp");
    libraw.file("LibRaw/src/metadata/leica.cpp");
    libraw.file("LibRaw/src/metadata/makernotes.cpp");
    libraw.file("LibRaw/src/metadata/mediumformat.cpp");
    libraw.file("LibRaw/src/metadata/minolta.cpp");
    libraw.file("LibRaw/src/metadata/misc_parsers.cpp");
    libraw.file("LibRaw/src/metadata/nikon.cpp");
    libraw.file("LibRaw/src/metadata/normalize_model.cpp");
    libraw.file("LibRaw/src/metadata/olympus.cpp");
    libraw.file("LibRaw/src/metadata/p1.cpp");
    libraw.file("LibRaw/src/metadata/pentax.cpp");
    libraw.file("LibRaw/src/metadata/samsung.cpp");
    libraw.file("LibRaw/src/metadata/sony.cpp");
    libraw.file("LibRaw/src/metadata/tiff.cpp");
    libraw.file("LibRaw/src/postprocessing/aspect_ratio.cpp");
    libraw.file("LibRaw/src/postprocessing/dcraw_process.cpp");
    libraw.file("LibRaw/src/postprocessing/mem_image.cpp");
    libraw.file("LibRaw/src/postprocessing/postprocessing_aux.cpp");
    libraw.file("LibRaw/src/postprocessing/postprocessing_utils.cpp");
    libraw.file("LibRaw/src/postprocessing/postprocessing_utils_dcrdefs.cpp");
    libraw.file("LibRaw/src/preprocessing/ext_preprocess.cpp");
    libraw.file("LibRaw/src/preprocessing/raw2image.cpp");
    libraw.file("LibRaw/src/preprocessing/subtract_black.cpp");
    libraw.file("LibRaw/src/tables/cameralist.cpp");
    libraw.file("LibRaw/src/tables/colorconst.cpp");
    libraw.file("LibRaw/src/tables/colordata.cpp");
    libraw.file("LibRaw/src/tables/wblists.cpp");
    libraw.file("LibRaw/src/utils/curves.cpp");
    libraw.file("LibRaw/src/utils/decoder_info.cpp");
    libraw.file("LibRaw/src/utils/init_close_utils.cpp");
    libraw.file("LibRaw/src/utils/open.cpp");
    libraw.file("LibRaw/src/utils/phaseone_processing.cpp");
    libraw.file("LibRaw/src/utils/read_utils.cpp");
    libraw.file("LibRaw/src/utils/thumb_utils.cpp");
    libraw.file("LibRaw/src/utils/utils_dcraw.cpp");
    libraw.file("LibRaw/src/utils/utils_libraw.cpp");
    libraw.file("LibRaw/src/write/apply_profile.cpp");
    libraw.file("LibRaw/src/write/file_write.cpp");
    libraw.file("LibRaw/src/write/tiff_writer.cpp");
    libraw.file("LibRaw/src/x3f/x3f_parse_process.cpp");
    libraw.file("LibRaw/src/x3f/x3f_utils_patched.cpp");
    libraw.file("LibRaw/src/libraw_c_api.cpp");
    libraw.file("LibRaw/src/libraw_datastream.cpp");

    libraw.warnings(false);
    libraw.extra_warnings(false);

    // GCC/Clang-only flags
    libraw.flag_if_supported("-Wno-format-truncation");
    libraw.flag_if_supported("-Wno-unused-result");
    libraw.flag_if_supported("-Wno-format-overflow");
    libraw.flag_if_supported("-Wno-deprecated-declarations");

    // -pthread is Unix-specific; MSVC/clang-cl do not want it.
    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        libraw.flag_if_supported("-pthread");
    }

    // MSVC: use multi-threaded static CRT matching Rust, silence some noise
    if target.contains("msvc") {
        libraw.flag_if_supported("/EHsc");
        libraw.flag_if_supported("/utf-8");
    }

    libraw.compile("raw");

    println!(
        "cargo:rustc-link-search=native={}",
        out_dir.as_ref().join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=raw");

    // C++ runtime for LibRaw
    if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-lib=dylib=msvcrt");
        // LibRaw may need these Windows system libs for file I/O
        println!("cargo:rustc-link-lib=dylib=ws2_32");
    } else if target.contains("apple") {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if !target.contains("windows") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
        println!("cargo:rustc-link-lib=dylib=pthread");
    }

    // Also link C++ stdlib when using clang on Windows (libc++)
    if target.contains("windows") {
        // clang++ often still uses MSVC STL on windows-msvc target
        println!("cargo:rustc-link-lib=dylib=shell32");
        println!("cargo:rustc-link-lib=dylib=ole32");
    }
}

fn bindings(out_dir: impl AsRef<Path>) {
    let path = out_dir.as_ref().join("bindings.rs");
    let host = env::var("HOST").expect("HOST is not set");
    let target = env::var("TARGET").expect("TARGET is not set");

    // Cross builds may not have a usable target sysroot for clang/bindgen.
    if host != target {
        fs::copy("src/bindings.rs", &path).expect("Couldn't copy pregenerated bindings");
        return;
    }

    let bindings = bindgen::Builder::default()
        .header("LibRaw/libraw/libraw.h")
        .clang_arg("-DLIBRAW_NODLL")
        .use_core()
        .ctypes_prefix("libc")
        .generate_comments(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .layout_tests(false)
        .derive_eq(true)
        .no_partialeq("__darwin_pthread_handler_rec")
        .no_partialeq("_IO_FILE")
        .no_partialeq("_IO_cookie_io_functions_t")
        .no_partialeq("sigvec")
        .no_partialeq("libraw_callbacks_t")
        .no_partialeq("__sFILE")
        .size_t_is_usize(true)
        .blocklist_function("_.*")
        .blocklist_item("FP_NAN")
        .blocklist_item("FP_INFINITE")
        .blocklist_item("FP_ZERO")
        .blocklist_item("FP_SUBNORMAL")
        .blocklist_item("FP_NORMAL")
        .blocklist_function("acoshl")
        .blocklist_function("acosl")
        .blocklist_function("asinhl")
        .blocklist_function("asinl")
        .blocklist_function("atan2l")
        .blocklist_function("atanhl")
        .blocklist_function("atanl")
        .blocklist_function("cbrtl")
        .blocklist_function("ceill")
        .blocklist_function("copysignl")
        .blocklist_function("coshl")
        .blocklist_function("cosl")
        .blocklist_function("dreml")
        .blocklist_function("ecvt_r")
        .blocklist_function("erfcl")
        .blocklist_function("erfl")
        .blocklist_function("exp2l")
        .blocklist_function("expl")
        .blocklist_function("expm1l")
        .blocklist_function("fabsl")
        .blocklist_function("fcvt_r")
        .blocklist_function("fdiml")
        .blocklist_function("finitel")
        .blocklist_function("floorl")
        .blocklist_function("fmal")
        .blocklist_function("fmaxl")
        .blocklist_function("fminl")
        .blocklist_function("fmodl")
        .blocklist_function("frexpl")
        .blocklist_function("gammal")
        .blocklist_function("hypotl")
        .blocklist_function("ilogbl")
        .blocklist_function("isinfl")
        .blocklist_function("isnanl")
        .blocklist_function("j0l")
        .blocklist_function("j1l")
        .blocklist_function("jnl")
        .blocklist_function("ldexpl")
        .blocklist_function("lgammal")
        .blocklist_function("lgammal_r")
        .blocklist_function("llrintl")
        .blocklist_function("llroundl")
        .blocklist_function("log10l")
        .blocklist_function("log1pl")
        .blocklist_function("log2l")
        .blocklist_function("logbl")
        .blocklist_function("logl")
        .blocklist_function("lrintl")
        .blocklist_function("lroundl")
        .blocklist_function("modfl")
        .blocklist_function("nanl")
        .blocklist_function("nearbyintl")
        .blocklist_function("nextafterl")
        .blocklist_function("nexttoward")
        .blocklist_function("nexttowardf")
        .blocklist_function("nexttowardl")
        .blocklist_function("powl")
        .blocklist_function("qecvt")
        .blocklist_function("qecvt_r")
        .blocklist_function("qfcvt")
        .blocklist_function("qfcvt_r")
        .blocklist_function("qgcvt")
        .blocklist_function("remainderl")
        .blocklist_function("remquol")
        .blocklist_function("rintl")
        .blocklist_function("roundl")
        .blocklist_function("scalbl")
        .blocklist_function("scalblnl")
        .blocklist_function("scalbnl")
        .blocklist_function("significandl")
        .blocklist_function("sinhl")
        .blocklist_function("sinl")
        .blocklist_function("sqrtl")
        .blocklist_function("strtold")
        .blocklist_function("tanhl")
        .blocklist_function("tanl")
        .blocklist_function("tgammal")
        .blocklist_function("truncl")
        .blocklist_function("y0l")
        .blocklist_function("y1l")
        .blocklist_function("ynl")
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(path)
        .expect("Couldn't write bindings!");
}
