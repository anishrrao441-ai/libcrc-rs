//! Input generation and the on-the-wire case format.
//!
//! Every case is addressed by a `u64` index. Index *N* under seed *S* always produces the
//! same bytes, whatever the batch size and whatever order batches ran in — see
//! [`crate::rng::Rng::for_case`]. That is what makes the recorded seed in `fuzz/log.txt`
//! a genuine replay handle rather than a decoration.
//!
//! Indices below [`prologue`]`().len()` are a fixed, hand-picked corpus. Everything above
//! is drawn from the weighted classes in [`Class`]. The fixed corpus exists so that the
//! coverage claims (every byte value, every word/table boundary, 64 KiB+ buffers, NMEA
//! delimiter torture, the NULL-pointer contract) hold on *every* run rather than
//! probabilistically.

use crate::rng::Rng;

/// Case-file magic. `results.bin` uses `PMFR`.
pub const MAGIC_CASES: [u8; 4] = *b"PMFZ";
pub const FORMAT_VERSION: u32 = 1;

/// Lengths that sit on or beside a table, word or cache boundary — where an off-by-one in
/// a loop bound or a tail-handling branch would surface.
pub const BOUNDARY_LENS: [u32; 16] = [
    1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129,
];

/// Where the input came from. Tracked only so the log can report the mix honestly.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Class {
    Prologue,
    Empty,
    SingleByte,
    BoundaryLen,
    RandomSmall,
    RandomMedium,
    UniformByte,
    ZeroHeavy,
    FfHeavy,
    Sparse,
    NmeaSentence,
    DelimiterAscii,
    LongBuffer,
    NullPointer,
}

impl Class {
    pub const ALL: [Class; 14] = [
        Class::Prologue,
        Class::Empty,
        Class::SingleByte,
        Class::BoundaryLen,
        Class::RandomSmall,
        Class::RandomMedium,
        Class::UniformByte,
        Class::ZeroHeavy,
        Class::FfHeavy,
        Class::Sparse,
        Class::NmeaSentence,
        Class::DelimiterAscii,
        Class::LongBuffer,
        Class::NullPointer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Class::Prologue => "fixed-corpus",
            Class::Empty => "empty",
            Class::SingleByte => "single-byte",
            Class::BoundaryLen => "boundary-length",
            Class::RandomSmall => "random-binary-small",
            Class::RandomMedium => "random-binary-medium",
            Class::UniformByte => "uniform-byte-fill",
            Class::ZeroHeavy => "zero-heavy",
            Class::FfHeavy => "0xFF-heavy",
            Class::Sparse => "sparse",
            Class::NmeaSentence => "nmea-sentence",
            Class::DelimiterAscii => "delimiter-rich-ascii",
            Class::LongBuffer => "long-buffer-16KiB..256KiB",
            Class::NullPointer => "null-pointer",
        }
    }
}

/// One generated input.
#[derive(Clone)]
pub struct Case {
    /// Pass a NULL pointer to the C API instead of the payload. libcrc guards every loop
    /// with `if ( ptr != NULL )` and returns the init value, and `checksum_NMEA` returns
    /// NULL outright — both are observable behaviour the port has to reproduce.
    pub is_null: bool,
    pub data: Vec<u8>,
}

impl Case {
    fn new(data: Vec<u8>) -> Self {
        Case { is_null: false, data }
    }
}

/// Where one case's payload lives inside a batch blob.
pub struct Span {
    pub index: u64,
    pub offset: usize,
    pub len: usize,
    pub is_null: bool,
    pub class: Class,
}

/// A batch, already serialised into the exact bytes the oracle will read.
pub struct Batch {
    pub blob: Vec<u8>,
    pub spans: Vec<Span>,
}

impl Batch {
    /// The payload for one case, as the port will see it. A NULL case yields the empty
    /// slice, which is precisely how the C-ABI shim maps a NULL pointer.
    pub fn payload<'a>(&'a self, span: &Span) -> &'a [u8] {
        if span.is_null {
            &[]
        } else {
            &self.blob[span.offset..span.offset + span.len]
        }
    }
}

// ===========================================================================
// Serialisation
// ===========================================================================

struct BatchWriter {
    blob: Vec<u8>,
    spans: Vec<Span>,
    /// Generators build into this, never into `blob`.
    ///
    /// The first version let them append straight to `blob` to save a memcpy. That put a
    /// generator one arithmetic slip away from writing *behind* its own payload: some
    /// classes poke a byte at a random offset, and an offset taken against the whole blob
    /// instead of the current payload silently overwrote an earlier case's length field.
    /// The batch then failed to parse thousands of cases later, nowhere near the cause.
    /// Handing generators a buffer that starts at zero and contains nothing else makes
    /// that class of bug unrepresentable; the memcpy is noise next to a process spawn.
    scratch: Vec<u8>,
}

impl BatchWriter {
    fn with_capacity(cases: usize) -> Self {
        BatchWriter {
            // 12-byte header, then ~5 bytes of framing plus a typical payload per case.
            blob: Vec::with_capacity(12 + cases * 160),
            spans: Vec::with_capacity(cases),
            scratch: Vec::with_capacity(262_144),
        }
    }

    fn begin(&mut self) {
        self.blob.extend_from_slice(&MAGIC_CASES);
        self.blob.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        self.blob.extend_from_slice(&0u32.to_le_bytes()); // patched by finish()
    }

    fn push<F>(&mut self, index: u64, class: Class, is_null: bool, write_payload: F)
    where
        F: FnOnce(&mut Vec<u8>),
    {
        self.scratch.clear();
        write_payload(&mut self.scratch);

        let len = self.scratch.len();
        self.blob.push(u8::from(is_null));
        self.blob.extend_from_slice(&(len as u32).to_le_bytes());
        let offset = self.blob.len();
        self.blob.extend_from_slice(&self.scratch);

        self.spans.push(Span { index, offset, len, is_null, class });
    }

    fn finish(mut self) -> Batch {
        let count = self.spans.len() as u32;
        self.blob[8..12].copy_from_slice(&count.to_le_bytes());
        Batch { blob: self.blob, spans: self.spans }
    }
}

// ===========================================================================
// The fixed corpus
// ===========================================================================

fn repeated(byte: u8, len: usize) -> Vec<u8> {
    vec![byte; len]
}

fn counting(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i & 0xFF) as u8).collect()
}

fn alternating(len: usize) -> Vec<u8> {
    (0..len).map(|i| if i % 2 == 0 { 0x55 } else { 0xAA }).collect()
}

/// Deterministic corpus, evaluated once per process. Covers, by construction:
/// empty input; all 256 byte values as a single byte; every length in
/// [`BOUNDARY_LENS`] under four fill patterns; all-zero and all-`0xFF` buffers across
/// three orders of magnitude; a uniform buffer for each of the 256 byte values; buffers
/// at and around 64 KiB plus one 256 KiB buffer; NMEA sentences with and without the
/// leading `$` and with every terminator libcrc recognises; embedded NULs; and the
/// NULL-pointer contract at four lengths.
pub fn prologue() -> Vec<Case> {
    let mut out: Vec<Case> = Vec::new();

    // Empty.
    out.push(Case::new(Vec::new()));

    // Every byte value on its own.
    for b in 0u16..=255 {
        out.push(Case::new(vec![b as u8]));
    }

    // Boundary lengths under four fill patterns.
    for &len in BOUNDARY_LENS.iter() {
        let len = len as usize;
        out.push(Case::new(repeated(0x00, len)));
        out.push(Case::new(repeated(0xFF, len)));
        out.push(Case::new(counting(len)));
        out.push(Case::new(alternating(len)));
    }

    // All-zero and all-0xFF across magnitudes, including exact powers of two and their
    // immediate neighbours.
    const MAGNITUDES: [usize; 11] = [0, 1, 255, 256, 257, 1023, 1024, 1025, 4095, 4096, 4097];
    for &len in MAGNITUDES.iter() {
        out.push(Case::new(repeated(0x00, len)));
        out.push(Case::new(repeated(0xFF, len)));
    }

    // A uniform buffer for each byte value: catches a table index that is right for the
    // check string but wrong for some other entry.
    for b in 0u16..=255 {
        out.push(Case::new(repeated(b as u8, 17)));
    }

    // Long buffers. 64 KiB is the stated floor; go past it and straddle it.
    const K64: usize = 65_536;
    out.push(Case::new(repeated(0x00, K64)));
    out.push(Case::new(repeated(0xFF, K64)));
    out.push(Case::new(counting(K64)));
    out.push(Case::new(alternating(K64)));
    out.push(Case::new(counting(K64 - 1)));
    out.push(Case::new(counting(K64 + 1)));
    out.push(Case::new(counting(262_144)));

    // NMEA. checksum_NMEA is the one delimiter-driven function in the library: it skips a
    // leading '$' and halts on NUL, CR, LF or '*'. Each of those exits gets its own case,
    // in both the with-$ and without-$ forms.
    let bodies: [&str; 8] = [
        "GPGLL,4916.45,N,12311.12,W,225444,A",
        "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,",
        "GPRMC,081836,A,3751.65,S,14507.36,E,000.0,360.0,130998,011.3,E",
        "PGRME,15.0,M,45.0,M,25.0,M",
        "A",
        "",
        ",,,,,",
        "GPVTG,054.7,T,034.4,M,005.5,N,010.2,K",
    ];
    let terminators: [&str; 6] = ["", "*7C", "*7C\r\n", "\r\n", "\r", "\n"];
    for body in bodies.iter() {
        for term in terminators.iter() {
            out.push(Case::new(format!("{body}{term}").into_bytes()));
            out.push(Case::new(format!("${body}{term}").into_bytes()));
        }
    }

    // Delimiters in adversarial positions, including embedded NULs and a lone '$'.
    let torture: [&[u8]; 12] = [
        b"$",
        b"*",
        b"$*",
        b"\r",
        b"\n",
        b"$\r\n",
        b"\0",
        b"$\0ABC",
        b"AB\0CD",
        b"AB*CD",
        b"$$GPGLL",
        b"$GPGLL*",
    ];
    for t in torture.iter() {
        out.push(Case::new(t.to_vec()));
    }
    // A NUL in the middle of an otherwise ordinary sentence: everything after it is
    // invisible to checksum_NMEA but still hashed by the twelve length-driven CRCs.
    out.push(Case::new(b"$GPGLL,4916.45\0,N,12311.12,W*7C\r\n".to_vec()));

    // The NULL-pointer contract, at four lengths.
    for &len in [0u32, 1, 16, 1024].iter() {
        out.push(Case { is_null: true, data: repeated(0xAB, len as usize) });
    }

    out
}

// ===========================================================================
// The random classes
// ===========================================================================

/// Weighted class selection, out of 10 000.
///
/// Long buffers are deliberately rare: at 21 CRCs per case a 256 KiB input costs about
/// 5 MiB of folding, so a high rate would trade a lot of case count for little extra
/// signal. Guaranteed long-buffer coverage lives in the fixed corpus instead.
fn pick_class(rng: &mut Rng) -> Class {
    match rng.below(10_000) {
        0..=2_999 => Class::RandomSmall,
        3_000..=4_499 => Class::RandomMedium,
        4_500..=5_499 => Class::BoundaryLen,
        5_500..=6_499 => Class::SingleByte,
        6_500..=7_299 => Class::NmeaSentence,
        7_300..=8_099 => Class::UniformByte,
        8_100..=8_599 => Class::Sparse,
        8_600..=9_099 => Class::DelimiterAscii,
        9_100..=9_499 => Class::Empty,
        9_500..=9_899 => Class::FfHeavy,
        9_900..=9_939 => Class::ZeroHeavy,
        9_940..=9_989 => Class::NullPointer,
        _ => Class::LongBuffer,
    }
}

const DELIMITER_ALPHABET: &[u8] = b"$*\r\n\0,.-0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";

fn write_random_class(out: &mut Vec<u8>, class: Class, rng: &mut Rng) {
    match class {
        Class::Empty => {}

        Class::SingleByte => out.push(rng.next_u8()),

        Class::BoundaryLen => {
            let len = BOUNDARY_LENS[rng.below(BOUNDARY_LENS.len() as u32) as usize] as usize;
            let at = out.len();
            out.resize(at + len, 0);
            rng.fill(&mut out[at..]);
        }

        Class::RandomSmall => {
            let len = rng.below(257) as usize;
            let at = out.len();
            out.resize(at + len, 0);
            rng.fill(&mut out[at..]);
        }

        Class::RandomMedium => {
            let len = rng.below(4097) as usize;
            let at = out.len();
            out.resize(at + len, 0);
            rng.fill(&mut out[at..]);
        }

        Class::UniformByte => {
            let len = rng.below(1025) as usize;
            let byte = rng.next_u8();
            out.resize(out.len() + len, byte);
        }

        Class::ZeroHeavy => out.resize(out.len() + rng.below(2049) as usize, 0x00),

        Class::FfHeavy => {
            let len = rng.below(2049) as usize;
            let at = out.len();
            out.resize(at + len, 0xFF);
            // A few bytes knocked out of an otherwise saturated buffer.
            let flips = rng.below(5);
            for _ in 0..flips {
                if len > 0 {
                    let pos = at + rng.below(len as u32) as usize;
                    out[pos] = rng.next_u8();
                }
            }
        }

        Class::Sparse => {
            let len = rng.below(1025) as usize;
            let at = out.len();
            out.resize(at + len, 0x00);
            let hits = rng.below(6);
            for _ in 0..hits {
                if len > 0 {
                    let pos = at + rng.below(len as u32) as usize;
                    out[pos] = rng.next_u8();
                }
            }
        }

        Class::DelimiterAscii => {
            let len = rng.below(129) as usize;
            for _ in 0..len {
                let pick = rng.below(DELIMITER_ALPHABET.len() as u32) as usize;
                out.push(DELIMITER_ALPHABET[pick]);
            }
        }

        Class::NmeaSentence => write_nmea(out, rng),

        Class::LongBuffer => {
            let len = rng.range(16_384, 262_144) as usize;
            let at = out.len();
            out.resize(at + len, 0);
            match rng.below(4) {
                0 => {} // all zero
                1 => out[at..].fill(0xFF),
                2 => {
                    for (i, slot) in out[at..].iter_mut().enumerate() {
                        *slot = (i & 0xFF) as u8;
                    }
                }
                _ => rng.fill(&mut out[at..]),
            }
        }

        // A NULL case still carries a payload on the wire; the oracle reads it and then
        // passes NULL anyway. Keeping the framing uniform is worth a few wasted bytes on
        // 0.5% of cases.
        Class::NullPointer => {
            let len = rng.below(64) as usize;
            let at = out.len();
            out.resize(at + len, 0);
            rng.fill(&mut out[at..]);
        }

        Class::Prologue => unreachable!("the fixed corpus is emitted verbatim, not generated"),
    }
}

/// Synthesise something NMEA-shaped: optional `$`, a talker/sentence id, comma-separated
/// fields, and — often but not always — a `*` checksum suffix and a CR/LF terminator.
/// Sometimes a stray NUL or delimiter is injected mid-sentence.
fn write_nmea(out: &mut Vec<u8>, rng: &mut Rng) {
    // Every index below is taken relative to `at`, never to the raw buffer length.
    let at = out.len();

    if rng.below(4) != 0 {
        out.push(b'$');
    }

    for _ in 0..5 {
        out.push(b'A' + rng.below(26) as u8);
    }

    let fields = rng.below(9);
    for _ in 0..fields {
        out.push(b',');
        let width = rng.below(9);
        for _ in 0..width {
            out.push(match rng.below(12) {
                0..=7 => b'0' + rng.below(10) as u8,
                8 => b'.',
                9 => b'-',
                10 => b'A' + rng.below(26) as u8,
                _ => b'a' + rng.below(26) as u8,
            });
        }
    }

    // Inject a terminator character somewhere in the middle now and then, so the early
    // exits are hit at an interior offset rather than only at the end.
    if rng.below(8) == 0 && out.len() > at {
        let pos = at + rng.below((out.len() - at) as u32) as usize;
        out[pos] = match rng.below(4) {
            0 => 0x00,
            1 => b'\r',
            2 => b'\n',
            _ => b'*',
        };
    }

    if rng.below(3) != 0 {
        out.push(b'*');
        let sum = rng.next_u8();
        out.extend_from_slice(format!("{sum:02X}").as_bytes());
    }

    match rng.below(4) {
        0 => out.extend_from_slice(b"\r\n"),
        1 => out.push(b'\r'),
        2 => out.push(b'\n'),
        _ => {}
    }
}

// ===========================================================================
// Batch assembly
// ===========================================================================

fn append_case(w: &mut BatchWriter, fixed: &[Case], seed: u64, index: u64) {
    if let Some(case) = fixed.get(index as usize) {
        let is_null = case.is_null;
        let data = &case.data;
        w.push(index, Class::Prologue, is_null, |out| out.extend_from_slice(data));
        return;
    }

    let mut rng = Rng::for_case(seed, index);
    let class = pick_class(&mut rng);
    w.push(index, class, class == Class::NullPointer, |out| {
        write_random_class(out, class, &mut rng)
    });
}

/// Build `count` cases starting at `start_index`.
pub fn build_batch(fixed: &[Case], seed: u64, start_index: u64, count: usize) -> Batch {
    let mut w = BatchWriter::with_capacity(count);
    w.begin();
    for i in 0..count as u64 {
        append_case(&mut w, fixed, seed, start_index + i);
    }
    w.finish()
}

/// A one-case batch built from explicit bytes. Used by the shrinker, which needs to ask
/// the oracle about inputs that no seed would ever produce.
pub fn build_literal(data: &[u8], is_null: bool) -> Batch {
    let mut w = BatchWriter::with_capacity(1);
    w.begin();
    w.push(u64::MAX, Class::Prologue, is_null, |out| out.extend_from_slice(data));
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_bytes_depend_only_on_seed_and_index() {
        let fixed = prologue();
        // Same indices, reached through different batch sizes.
        let wide = build_batch(&fixed, 99, 5_000, 8);
        let narrow = build_batch(&fixed, 99, 5_000, 8);
        let offset = build_batch(&fixed, 99, 5_004, 4);

        for i in 0..8 {
            assert_eq!(wide.payload(&wide.spans[i]), narrow.payload(&narrow.spans[i]));
        }
        for i in 0..4 {
            assert_eq!(wide.payload(&wide.spans[i + 4]), offset.payload(&offset.spans[i]));
        }
    }

    /// Walk the blob exactly the way the C harness does, and confirm every framed length
    /// still matches the span recorded at generation time.
    ///
    /// Regression test. The first version of `BatchWriter` let generators append directly
    /// to the batch blob, and `write_nmea` picked its "inject a delimiter mid-sentence"
    /// offset against the whole blob rather than its own payload — so it could overwrite
    /// a *previous* case's 4-byte length field. The oracle then rejected the batch with
    /// "truncated payload for case 159" 10 000 cases into a run, pointing nowhere near
    /// the generator that did it. The ranges below are chosen to include NMEA cases.
    #[test]
    fn wire_format_parses_end_to_end() {
        let fixed = prologue();
        for (start, count) in [(0u64, 800usize), (5_000, 5_000), (10_000, 5_000)] {
            let batch = build_batch(&fixed, 0x1234, start, count);
            assert_eq!(&batch.blob[0..4], &MAGIC_CASES);
            assert_eq!(
                u32::from_le_bytes(batch.blob[4..8].try_into().unwrap()),
                FORMAT_VERSION
            );
            assert_eq!(
                u32::from_le_bytes(batch.blob[8..12].try_into().unwrap()) as usize,
                count
            );

            let mut pos = 12usize;
            for (i, span) in batch.spans.iter().enumerate() {
                assert!(pos + 5 <= batch.blob.len(), "header of case {i} runs off the end");
                let framed = u32::from_le_bytes(batch.blob[pos + 1..pos + 5].try_into().unwrap());
                assert_eq!(
                    framed as usize, span.len,
                    "case {i} (index {}) framed length {framed} != span length {}",
                    span.index, span.len
                );
                assert_eq!(batch.blob[pos] & 0x01 != 0, span.is_null, "case {i} null flag");
                assert_eq!(pos + 5, span.offset, "case {i} payload offset");
                pos += 5 + span.len;
                assert!(pos <= batch.blob.len(), "payload of case {i} runs off the end");
            }
            assert_eq!(pos, batch.blob.len(), "trailing bytes after the last case");
        }
    }

    /// A generator must never touch anything outside the buffer it was handed.
    #[test]
    fn generators_only_append_to_their_own_payload() {
        for index in 0..20_000u64 {
            let mut rng = Rng::for_case(0xABCD_EF01, index);
            let class = pick_class(&mut rng);
            if class == Class::Prologue {
                continue;
            }
            // A canary prefix standing in for previously written cases.
            const CANARY: &[u8] = b"\xDE\xAD\xBE\xEF-do-not-touch-me-0123456789";
            let mut buf = CANARY.to_vec();
            write_random_class(&mut buf, class, &mut rng);
            assert_eq!(
                &buf[..CANARY.len()],
                CANARY,
                "class {:?} at index {index} wrote behind its own payload",
                class
            );
        }
    }

    #[test]
    fn fixed_corpus_covers_what_it_claims() {
        let fixed = prologue();

        assert!(fixed.iter().any(|c| c.data.is_empty() && !c.is_null), "empty input");
        for b in 0u16..=255 {
            assert!(
                fixed.iter().any(|c| c.data.len() == 1 && c.data[0] == b as u8),
                "single byte 0x{b:02X}"
            );
        }
        for &len in BOUNDARY_LENS.iter() {
            assert!(
                fixed.iter().any(|c| c.data.len() == len as usize),
                "boundary length {len}"
            );
        }
        assert!(fixed.iter().any(|c| c.data.len() >= 65_536), ">=64KiB buffer");
        assert!(fixed.iter().any(|c| c.data.len() >= 262_144), ">=256KiB buffer");
        assert!(
            fixed.iter().any(|c| c.data.len() > 64 && c.data.iter().all(|&b| b == 0)),
            "all-zero buffer"
        );
        assert!(
            fixed.iter().any(|c| c.data.len() > 64 && c.data.iter().all(|&b| b == 0xFF)),
            "all-0xFF buffer"
        );
        assert!(fixed.iter().any(|c| c.is_null), "NULL-pointer case");
        assert!(fixed.iter().any(|c| c.data.starts_with(b"$")), "NMEA with $");
        assert!(
            fixed.iter().any(|c| c.data.contains(&b'*') && !c.data.starts_with(b"$")),
            "NMEA without $, with *"
        );
        assert!(fixed.iter().any(|c| c.data.contains(&b'\r')), "CR terminator");
        assert!(fixed.iter().any(|c| c.data.contains(&b'\n')), "LF terminator");
        assert!(fixed.iter().any(|c| c.data.contains(&0u8)), "embedded NUL");
    }
}
