// Runs the guest under the RISC0 executor (no proving) and prints the cycle
// table. Executor only: these are the counts that decide whether an operation
// fits the LEZ public-execution budget, and they are deterministic.

use auction_methods::{AUCTION_GUEST_ELF, AUCTION_GUEST_ID};
use risc0_zkvm::{default_executor, ExecutorEnv};

type Row = (String, u64, u64, u64, usize);

fn main() {
    let env = ExecutorEnv::builder().build().unwrap();
    let info = default_executor().execute(env, AUCTION_GUEST_ELF).unwrap();

    let (rows, baseline, whole_bid): (Vec<Row>, u64, u64) = info.journal.decode().unwrap();

    println!("guest image id : {:?}", AUCTION_GUEST_ID);
    println!("total cycles   : {}", info.cycles());
    println!("measurement baseline subtracted from every row: {baseline} cycles\n");
    println!("| op | cases | median | min | max |");
    println!("|---|---:|---:|---:|---:|");
    for (name, med, lo, hi, n) in &rows {
        println!("| `{name}` | {n} | {med} | {lo} | {hi} |");
    }
    println!("\nwhole bid (quote then settle), end to end: {whole_bid} cycles");
}
