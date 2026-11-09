fn main() {
    folio_app::prepare();
    #[cfg(any(feature = "desktop", feature = "mobile"))]
    folio_app::launch();
    #[cfg(not(any(feature = "desktop", feature = "mobile")))]
    {
        println!("{}", folio_app::linked_crates().join(" "));
        std::thread::park();
    }
}
