#[tokio::main]
async fn main() {
    std::process::exit(netbird_hawk::cli::entrypoint().await);
}
