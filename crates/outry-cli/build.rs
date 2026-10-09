fn main() {
    // Target triple нужен `outry update`, чтобы выбрать архив своей платформы в релизе.
    println!(
        "cargo:rustc-env=OUTRY_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
