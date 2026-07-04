use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use proptest::prelude::*;

proptest! {
    #[test]
    fn executable_round_trips(
        arch in any::<u8>(),
        code in proptest::collection::vec(any::<u8>(), 1..512),
        entry_seed in any::<u32>(),
    ) {
        let entry = entry_seed % code.len() as u32;
        let exe = Executable { arch, entry, code };
        let back = Executable::from_bytes(&exe.to_bytes()).unwrap();
        prop_assert_eq!(back, exe);
    }

    #[test]
    fn executable_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = Executable::from_bytes(&noise); // must return Err, not panic
    }

    #[test]
    fn object_round_trips(
        blob in proptest::collection::vec(any::<u8>(), 5..64),
        name in "[a-zA-Z_][a-zA-Z0-9_]{0,12}",
        offset_seed in any::<u32>(),
    ) {
        let offset = offset_seed % (blob.len() as u32 - 4);
        let obj = ObjectFile {
            arch: 1,
            symbols: vec![
                Symbol { name: name.clone(), def: SymbolDef::Defined { blob: 0 } },
                Symbol { name: format!("{name}_ext"), def: SymbolDef::External },
            ],
            blobs: vec![blob],
            relocations: vec![Relocation { blob: 0, offset, symbol: 1 }],
            debug: None,
        };
        let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
        prop_assert_eq!(back, obj);
    }

    #[test]
    fn object_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = ObjectFile::from_bytes(&noise);
    }

    #[test]
    fn tapeblock_round_trips(
        origin in any::<i64>(),
        head in any::<i64>(),
        cells in proptest::collection::vec(0u8..2, 1..128),
    ) {
        let block = TapeBlockFile {
            alphabet: vec![" ".into(), "*".into()],
            tapes: vec![TapeSnapshot { origin, cells, head }],
        };
        let back = TapeBlockFile::from_bytes(&block.to_bytes()).unwrap();
        prop_assert_eq!(back, block);
    }

    #[test]
    fn tapeblock_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = TapeBlockFile::from_bytes(&noise);
    }
}
