use shredstream::ShredListener;

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8001);

    let mut listener = ShredListener::bind(port).expect("failed to bind");
    eprintln!("Listening on 0.0.0.0:{port}");

    for (slot, transactions) in listener.transactions() {
        for tx in &transactions {
            println!("{}", tx.signatures[0]);
        }
        eprintln!("slot={slot} txs={}", transactions.len());
    }
}
