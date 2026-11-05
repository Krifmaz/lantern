//! The desktop app's binary: everything is in the `desktop` library (src/lib.rs), so the tests reach it.

fn main() -> iced::Result {
    desktop::run()
}
