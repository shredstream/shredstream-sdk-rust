use shredstream::ShredListener;
use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;
use std::time::{SystemTime, UNIX_EPOCH};

const PUMPFUN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");

const CREATE_DISC: [u8; 8] = [24, 30, 200, 40, 5, 28, 7, 119];
const CREATE_V2_DISC: [u8; 8] = [214, 144, 76, 236, 95, 139, 49, 180];

struct PumpfunCreate {
    mint: Pubkey,
    bonding_curve: Pubkey,
    creator: Pubkey,
}

fn detect_create(tx: &VersionedTransaction) -> Option<PumpfunCreate> {
    let keys = tx.message.static_account_keys();
    for ix in tx.message.instructions() {
        let Some(&program_id) = keys.get(ix.program_id_index as usize) else {
            continue;
        };
        if program_id != PUMPFUN_PROGRAM_ID {
            continue;
        }
        if ix.data.len() < 8 {
            continue;
        }
        let disc: [u8; 8] = ix.data[..8].try_into().unwrap();
        if disc != CREATE_DISC && disc != CREATE_V2_DISC {
            continue;
        }
        let resolve = |idx: usize| -> Pubkey {
            if idx < ix.accounts.len() {
                keys.get(ix.accounts[idx] as usize).copied().unwrap_or_default()
            } else {
                Pubkey::default()
            }
        };
        let is_v2 = disc == CREATE_V2_DISC;
        return Some(PumpfunCreate {
            mint: resolve(0),
            bonding_curve: resolve(2),
            creator: resolve(if is_v2 { 5 } else { 7 }),
        });
    }
    None
}

fn print_card(slot: u64, sig: &str, create: &PumpfunCreate) {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let total_secs = dur.as_secs();
    let ms = dur.subsec_millis();
    let h = (total_secs / 3600) % 24;
    let m = (total_secs / 60) % 60;
    let s = total_secs % 60;
    let time = format!("{h:02}:{m:02}:{s:02}.{ms:03}");

    let mint = create.mint.to_string();
    let curve = create.bonding_curve.to_string();
    let creator = create.creator.to_string();
    let sig_short = format!("{}...{}", &sig[..4], &sig[sig.len() - 4..]);

    const G: &str = "\x1b[1;32m";
    const DIM: &str = "\x1b[90m";
    const W: &str = "\x1b[97m";
    const Y: &str = "\x1b[33m";
    const C: &str = "\x1b[36m";
    const M: &str = "\x1b[35m";
    const D: &str = "\x1b[2m";
    const R: &str = "\x1b[0m";

    println!("{DIM}┌───────────────────────────────────────────────────────────────┐{R}");
    println!("{DIM}│{R}  🌐 {W}ShredStream.com{R} {DIM}SDK{R}                                       {DIM}│{R}");
    println!("{DIM}└───────────────────────────────────────────────────────────────┘{R}");
    println!();
    println!("{G}━━━━━━━━━━━━━━━━━━━━━━ 🚀 PUMPFUN CREATE ━━━━━━━━━━━━━━━━━━━━━━━{R}");
    println!(" {DIM}›{R} {DIM}🕐 Time{R}     {W}{time}{R}");
    println!(" {DIM}›{R} {DIM}📦 Slot{R}     {W}{slot}{R}");
    println!(" {DIM}›{R} {DIM}🪙 Mint{R}     {Y}{mint}{R}");
    println!(" {DIM}›{R} {DIM}📈 Curve{R}    {C}{curve}{R}");
    println!(" {DIM}›{R} {DIM}👤 Creator{R}  {M}{creator}{R}");
    println!(" {DIM}›{R} {DIM}🔑 Sig{R}      {D}{sig_short}{R}");
    println!("{G}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{R}");
}

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8001);

    let mut listener = ShredListener::bind(port).expect("failed to bind");
    eprintln!("Listening for PumpFun creates on 0.0.0.0:{port}");

    let mut found: u64 = 0;
    for (slot, transactions) in listener.transactions() {
        for tx in &transactions {
            if let Some(create) = detect_create(tx) {
                found += 1;
                let sig = tx.signatures[0].to_string();
                print!("\x1b[H\x1b[2J");
                print_card(slot, &sig, &create);
                println!("\n\x1b[90m  #{found} detected\x1b[0m");
            }
        }
    }
}
