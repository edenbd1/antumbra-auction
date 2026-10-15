// antumbra_auction (RFP-014) for LEZ v0.3.
//
// The whole program is `antumbra_auction_core::{plan, apply}`; this binary only
// hands them to the v0.3 entry point, which decodes the call, runs the one the
// orchestrator asked for, and writes the journal. See core/src/lib.rs for why
// the program is split the way it is.

fn main() {
    lee_core::program::run_program::<_, _, [u8], Vec<u8>>(
        antumbra_auction_core::plan,
        antumbra_auction_core::apply,
    )
}
