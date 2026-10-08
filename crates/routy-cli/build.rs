fn main() {
    // Target triple нужен `routy update`, чтобы выбрать архив своей платформы в релизе.
    println!(
        "cargo:rustc-env=ROUTY_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
