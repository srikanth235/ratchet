use ratchet_cli::io::ProcessIo;

#[tokio::main]
async fn main() {
    let code = ratchet_cli::run(std::env::args().collect(), &ProcessIo).await;
    std::process::exit(code);
}
