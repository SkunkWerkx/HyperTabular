//! The natives beside the core called directly through its C ABI, on memory the tests own
//! and pass by address as Java does; the registration table; and `JNI_OnLoad` against a stub
//! `JavaVM` and `JNIEnv`, on its success path and each of its three failures.

use super::*;
use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::ptr::{addr_of_mut, null};
use hypertabular::kernel::exports as core_abi;
use jni_sys::{JNIInvokeInterface_, JNINativeInterface_};

const ENV: *mut JNIEnv = null_mut();
const CLASS_ARG: jclass = null_mut();

/// The address Java would pass: `MemorySegment.address()` of native memory.
fn addr<T>(p: *const T) -> jlong {
    p.expose_provenance() as jlong
}

fn name_of(n: &Native) -> &str {
    core::str::from_utf8(&n.name[..n.name.len() - 1]).unwrap()
}

fn desc_of(n: &Native) -> &str {
    core::str::from_utf8(&n.descriptor[..n.descriptor.len() - 1]).unwrap()
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in snake.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Every export once, by the name the natives are registered under, with its descriptor:
/// pointers and `usize`s are `J`, `u32`s `I`, and the return the export's own width.
const EXPORTS: [(&str, &str); 23] = [
    ("hypertabular_version", "()I"),
    ("hypertabular_delimited_state_size", "()J"),
    ("hypertabular_delimited_init", "(JJ)I"),
    ("hypertabular_delimited_header", "(JJJIJJJJJ)I"),
    ("hypertabular_delimited_fill", "(JJJIJJJJJJ)I"),
    ("hypertabular_delimited_unescape", "(JJJJ)J"),
    ("hypertabular_workbook_state_size", "()J"),
    ("hypertabular_workbook_open", "(JJJJJ)I"),
    ("hypertabular_workbook_sheets", "(JJJJJ)I"),
    ("hypertabular_workbook_strings", "(JJJJJ)I"),
    ("hypertabular_workbook_styles", "(JJJJJ)I"),
    ("hypertabular_workbook_sheet", "(JJJJJIIIJ)I"),
    ("hypertabular_workbook_header", "(JJJJJ)I"),
    ("hypertabular_workbook_fill", "(JJJJJJJJJ)I"),
    ("hypertabular_delimited_init_with", "(JJ)I"),
    ("hypertabular_delimited_fill_with", "(JJJIJJJJJJ)I"),
    ("hypertabular_transcode", "(IJJIJJJ)I"),
    ("hypertabular_faults", "(JJJJIIJJJJ)I"),
    ("hypertabular_suggest", "(JJJJJJJJJJ)I"),
    ("hypertabular_workbook_sheet_with", "(JJJJJIJJ)I"),
    ("hypertabular_workbook_fill_with", "(JJJJJJJJJ)I"),
    ("hypertabular_range_parse", "(JJJ)I"),
    ("hypertabular_batch_check", "(JJJJIJ)I"),
];

#[test]
fn the_table_is_one_native_per_export_and_the_count() {
    assert_eq!(NATIVES.len(), EXPORTS.len() + 1);
    for (export, descriptor) in EXPORTS {
        let name = camel(export);
        let native = NATIVES
            .iter()
            .find(|n| name_of(n) == name)
            .unwrap_or_else(|| panic!("no native {name} for {export}"));
        assert_eq!(desc_of(native), descriptor, "{name}");
    }
    let last = &NATIVES[NATIVES.len() - 1];
    assert_eq!((name_of(last), desc_of(last)), ("jniNatives", "()I"));
    for n in &NATIVES {
        assert_eq!(n.name.last(), Some(&0));
        assert_eq!(n.descriptor.last(), Some(&0));
        assert!(!n.function.is_null());
    }
    assert!(names_unique(&NATIVES));
    // SAFETY: takes nothing it reads.
    assert_eq!(unsafe { native_jni_natives(ENV, CLASS_ARG) }, 24);
}

#[test]
fn the_table_is_rejected_with_a_repeat() {
    let twice = [
        native!("a", "()I", native_jni_natives),
        native!("a", "()J", native_hypertabular_delimited_state_size),
    ];
    assert!(!names_unique(&twice));
}

#[test]
fn version_and_sizes_are_the_cores() {
    // SAFETY: none of these take arguments.
    unsafe {
        assert_eq!(
            native_hypertabular_version(ENV, CLASS_ARG),
            core_abi::hypertabular_version() as jint
        );
        assert_eq!(
            native_hypertabular_delimited_state_size(ENV, CLASS_ARG),
            core_abi::hypertabular_delimited_state_size() as jlong
        );
        assert_eq!(
            native_hypertabular_workbook_state_size(ENV, CLASS_ARG),
            core_abi::hypertabular_workbook_state_size() as jlong
        );
    }
}

/// A state block of the core's size, 8-byte aligned.
fn state(size: usize) -> Vec<u64> {
    vec![0u64; size.div_ceil(8)]
}

/// What a header call leaves: its code, its `Filled`, the name spans and the arena.
type Header = (i32, [u64; 8], [u32; 8], [u8; 64]);

/// The header of one input through the native and through the export, side by side.
fn header_both(input: &[u8]) -> (Header, Header) {
    // SAFETY: takes no arguments.
    let size = unsafe { core_abi::hypertabular_delimited_state_size() };
    let dialect = [b',', 1, 0, 0];
    let run = |through_jni: bool| {
        let mut st = state(size);
        let mut names = [0u32; 8];
        let mut arena = [0u8; 64];
        let mut out = [0u64; 8];
        // SAFETY: every buffer is live and as large as the size passed beside it.
        let code = unsafe {
            if through_jni {
                assert_eq!(
                    native_hypertabular_delimited_init(
                        ENV,
                        CLASS_ARG,
                        addr(st.as_mut_ptr()),
                        addr(dialect.as_ptr())
                    ),
                    0
                );
                native_hypertabular_delimited_header(
                    ENV,
                    CLASS_ARG,
                    addr(st.as_mut_ptr()),
                    addr(input.as_ptr()),
                    input.len() as jlong,
                    1,
                    addr(names.as_mut_ptr()),
                    4,
                    addr(arena.as_mut_ptr()),
                    64,
                    addr(out.as_mut_ptr()),
                )
            } else {
                assert_eq!(
                    core_abi::hypertabular_delimited_init(
                        st.as_mut_ptr().cast(),
                        dialect.as_ptr().cast()
                    ),
                    0
                );
                core_abi::hypertabular_delimited_header(
                    st.as_mut_ptr().cast(),
                    input.as_ptr(),
                    input.len(),
                    1,
                    names.as_mut_ptr().cast(),
                    4,
                    arena.as_mut_ptr(),
                    64,
                    out.as_mut_ptr().cast(),
                )
            }
        };
        (code, out, names, arena)
    };
    (run(true), run(false))
}

#[test]
fn a_header_through_the_native_is_the_exports() {
    let (jni, direct) = header_both(b"id,\"na\"\"me\"\n1,x\n");
    assert_eq!(jni, direct);
    assert_eq!(jni.0, 0);
    // Two names.
    assert_eq!(jni.1[0], 2);
}

#[test]
fn a_bad_dialect_is_the_cores_refusal() {
    // SAFETY: takes no arguments.
    let size = unsafe { core_abi::hypertabular_delimited_state_size() };
    let mut st = state(size);
    let quote = [b'"', 1, 0, 0];
    // SAFETY: the state is the core's size, the dialect four bytes.
    let code = unsafe {
        native_hypertabular_delimited_init(
            ENV,
            CLASS_ARG,
            addr(st.as_mut_ptr()),
            addr(quote.as_ptr()),
        )
    };
    assert_eq!(code, -1);
    // And a null dialect, address 0, as the export answers it.
    // SAFETY: as above; null is the contract's own refusal.
    let null_dialect =
        unsafe { native_hypertabular_delimited_init(ENV, CLASS_ARG, addr(st.as_mut_ptr()), 0) };
    assert_eq!(null_dialect, -1);
}

#[test]
fn unescape_through_the_native_is_the_exports() {
    let cell = b"\"a\"\"b\"\"c\"";
    let mut jni = [0u8; 16];
    let mut direct = [0u8; 16];
    // SAFETY: each buffer is as large as its size.
    let (n, m) = unsafe {
        (
            native_hypertabular_delimited_unescape(
                ENV,
                CLASS_ARG,
                addr(cell.as_ptr()),
                cell.len() as jlong,
                addr(jni.as_mut_ptr()),
                16,
            ),
            core_abi::hypertabular_delimited_unescape(
                cell.as_ptr(),
                cell.len(),
                direct.as_mut_ptr(),
                16,
            ),
        )
    };
    assert_eq!(n as usize, m);
    assert_eq!(&jni[..n as usize], b"a\"b\"c");
    assert_eq!(jni, direct);
}

#[test]
fn every_workbook_call_on_a_non_workbook_is_the_exports() {
    // SAFETY: takes no arguments.
    let size = unsafe { core_abi::hypertabular_workbook_state_size() };
    let container = b"not a workbook at all";
    let mut window = vec![0u8; 64 * 1024];
    // Buffers: window and its size, then six null pointer-and-size pairs.
    let mut buffers = [0u64; 14];
    buffers[0] = window.as_mut_ptr().expose_provenance() as u64;
    buffers[1] = window.len() as u64;
    type Book =
        unsafe extern "system" fn(*mut JNIEnv, jclass, jlong, jlong, jlong, jlong, jlong) -> jint;
    type Direct = unsafe extern "C" fn(
        *mut hypertabular::kernel::workbook::State,
        *const u8,
        usize,
        *const hypertabular::kernel::abi::Buffers,
        *mut hypertabular::kernel::abi::Filled,
    ) -> i32;
    let open = |through_jni: bool| {
        let mut st = state(size);
        let mut out = [0u64; 8];
        // SAFETY: every buffer is live and as large as declared.
        let code = unsafe {
            if through_jni {
                native_hypertabular_workbook_open(
                    ENV,
                    CLASS_ARG,
                    addr(st.as_mut_ptr()),
                    addr(container.as_ptr()),
                    container.len() as jlong,
                    addr(buffers.as_ptr()),
                    addr(out.as_mut_ptr()),
                )
            } else {
                core_abi::hypertabular_workbook_open(
                    st.as_mut_ptr().cast(),
                    container.as_ptr(),
                    container.len(),
                    buffers.as_ptr().cast(),
                    out.as_mut_ptr().cast(),
                )
            }
        };
        (code, out, st)
    };
    let (code, out, st) = open(true);
    assert_eq!((code, out), {
        let (c, o, _) = open(false);
        (c, o)
    });
    assert_eq!(code, -2, "a non-workbook is a structural error");
    // The four calls that read an opened book, on the state the failed open left: each the
    // export's answer, whatever that is.
    let pairs: [(Book, Direct); 4] = [
        (
            native_hypertabular_workbook_sheets,
            core_abi::hypertabular_workbook_sheets,
        ),
        (
            native_hypertabular_workbook_strings,
            core_abi::hypertabular_workbook_strings,
        ),
        (
            native_hypertabular_workbook_styles,
            core_abi::hypertabular_workbook_styles,
        ),
        (
            native_hypertabular_workbook_header,
            core_abi::hypertabular_workbook_header,
        ),
    ];
    for (jni, direct) in pairs {
        let mut a = st.clone();
        let mut b = st.clone();
        let mut out_a = [0u64; 8];
        let mut out_b = [0u64; 8];
        // SAFETY: as above.
        let (x, y) = unsafe {
            (
                jni(
                    ENV,
                    CLASS_ARG,
                    addr(a.as_mut_ptr()),
                    addr(container.as_ptr()),
                    container.len() as jlong,
                    addr(buffers.as_ptr()),
                    addr(out_a.as_mut_ptr()),
                ),
                direct(
                    b.as_mut_ptr().cast(),
                    container.as_ptr(),
                    container.len(),
                    buffers.as_ptr().cast(),
                    out_b.as_mut_ptr().cast(),
                ),
            )
        };
        assert_eq!((x, out_a, a), (y, out_b, b));
    }
    // A sheet and a fill, null where the contract allows it: the export's refusal or answer.
    let mut a = st.clone();
    let mut b = st.clone();
    let mut out_a = [0u64; 8];
    let mut out_b = [0u64; 8];
    // SAFETY: as above; every other pointer is null with a zero size.
    let (x, y) = unsafe {
        (
            native_hypertabular_workbook_sheet(
                ENV,
                CLASS_ARG,
                addr(a.as_mut_ptr()),
                addr(container.as_ptr()),
                container.len() as jlong,
                0,
                0,
                0,
                1,
                0,
                addr(out_a.as_mut_ptr()),
            ),
            core_abi::hypertabular_workbook_sheet(
                b.as_mut_ptr().cast(),
                container.as_ptr(),
                container.len(),
                null(),
                0,
                0,
                1,
                0,
                out_b.as_mut_ptr().cast(),
            ),
        )
    };
    assert_eq!((x, out_a, &a), (y, out_b, &b));
    // SAFETY: as above.
    let (x, y) = unsafe {
        (
            native_hypertabular_workbook_fill(
                ENV,
                CLASS_ARG,
                addr(a.as_mut_ptr()),
                addr(container.as_ptr()),
                container.len() as jlong,
                0,
                0,
                0,
                1,
                addr(buffers.as_ptr()),
                addr(out_a.as_mut_ptr()),
            ),
            core_abi::hypertabular_workbook_fill(
                b.as_mut_ptr().cast(),
                container.as_ptr(),
                container.len(),
                null(),
                null(),
                0,
                1,
                buffers.as_ptr().cast(),
                out_b.as_mut_ptr().cast(),
            ),
        )
    };
    assert_eq!((x, out_a, a), (y, out_b, b));
}

#[test]
fn a_delimited_fill_with_nothing_to_fill_is_the_exports() {
    // SAFETY: takes no arguments.
    let size = unsafe { core_abi::hypertabular_delimited_state_size() };
    let dialect = [b',', 1, 0, 0];
    let input = b"1,2\n3,4\n";
    let run = |through_jni: bool| {
        let mut st = state(size);
        let mut out = [0u64; 8];
        // SAFETY: zero columns, so no spec, column or buffer is read.
        let code = unsafe {
            if through_jni {
                native_hypertabular_delimited_init(
                    ENV,
                    CLASS_ARG,
                    addr(st.as_mut_ptr()),
                    addr(dialect.as_ptr()),
                );
                native_hypertabular_delimited_fill(
                    ENV,
                    CLASS_ARG,
                    addr(st.as_mut_ptr()),
                    addr(input.as_ptr()),
                    input.len() as jlong,
                    1,
                    0,
                    0,
                    0,
                    8,
                    0,
                    addr(out.as_mut_ptr()),
                )
            } else {
                core_abi::hypertabular_delimited_init(
                    st.as_mut_ptr().cast(),
                    dialect.as_ptr().cast(),
                );
                core_abi::hypertabular_delimited_fill(
                    st.as_mut_ptr().cast(),
                    input.as_ptr(),
                    input.len(),
                    1,
                    null(),
                    null(),
                    0,
                    8,
                    null(),
                    out.as_mut_ptr().cast(),
                )
            }
        };
        (code, out, st)
    };
    assert_eq!(run(true), run(false));
}

// --- JNI_OnLoad against a stub VM ---

#[derive(Default)]
struct Load {
    get_env_fails: bool,
    find_class_fails: bool,
    register_fails: bool,
    found_class: String,
    env: usize,
    registered: Vec<(String, String, usize)>,
}

thread_local! {
    static LOAD: RefCell<Load> = RefCell::new(Load::default());
}

const CLASS_TOKEN: usize = 0x1000;

/// The bytes of a `Filled` result, zeroed, 8-byte aligned.
const FILLED_WORDS: usize = 8;

#[test]
fn the_batch_check_through_the_native_is_the_exports() {
    for (rows, buffer, flags) in [
        (16_u64, 1024_u64, 0_u32),
        (0, 1024, 0),
        (16, 0, 0),
        (1 << 30, 1024, 4),
        (u64::MAX, u64::MAX, 2),
    ] {
        let mut jni = [0xAAu64; 4];
        let mut direct = [0xAAu64; 4];
        // SAFETY: no specs, as a count of 0 says, and `out` is 32 bytes on 8-byte alignment.
        let (x, y) = unsafe {
            (
                native_hypertabular_batch_check(
                    ENV,
                    CLASS_ARG,
                    0,
                    0,
                    rows as jlong,
                    buffer as jlong,
                    flags as jint,
                    addr(jni.as_mut_ptr()),
                ),
                hypertabular_batch_check(
                    null(),
                    0,
                    rows,
                    buffer,
                    flags,
                    direct.as_mut_ptr().cast(),
                ),
            )
        };
        assert_eq!(x, y, "{rows} {buffer} {flags}");
        assert_eq!(jni, direct, "{rows} {buffer} {flags}");
    }
}

#[test]
fn the_range_parse_through_the_native_is_the_exports() {
    for text in [&b"B2:C3"[..], b"", b"nonsense", b"C3:B2", b"A1"] {
        let mut jni = [0xAAu32; 4];
        let mut direct = [0xAAu32; 4];
        // SAFETY: each pointer is live memory of the size beside it.
        let (x, y) = unsafe {
            (
                native_hypertabular_range_parse(
                    ENV,
                    CLASS_ARG,
                    addr(text.as_ptr()),
                    text.len() as jlong,
                    addr(jni.as_mut_ptr()),
                ),
                hypertabular_range_parse(
                    text.as_ptr(),
                    text.len(),
                    direct.as_mut_ptr().cast::<c_void>(),
                ),
            )
        };
        assert_eq!(x, y, "{text:?}");
        assert_eq!(jni, direct, "{text:?}");
    }
    let text = b"B2:C3";
    let mut out = [0u32; 4];
    // SAFETY: as above.
    let rc = unsafe {
        native_hypertabular_range_parse(
            ENV,
            CLASS_ARG,
            addr(text.as_ptr()),
            text.len() as jlong,
            addr(out.as_mut_ptr()),
        )
    };
    assert_eq!(rc, 0);
    assert_eq!(out, [2, 2, 3, 3]);
}

#[test]
fn a_transcode_through_the_native_is_the_exports() {
    // windows-1252: 0x80 is the euro sign. The last byte is one windows-1252 leaves undefined.
    for input in [&b"\x80a"[..], b"\x81"] {
        let mut out_a = [0u8; 32];
        let mut out_b = [0u8; 32];
        let mut progress_a = [0u64; 4];
        let mut progress_b = [0u64; 4];
        // SAFETY: each pointer is live memory of the size beside it.
        let (x, y) = unsafe {
            (
                native_hypertabular_transcode(
                    ENV,
                    CLASS_ARG,
                    1,
                    addr(input.as_ptr()),
                    input.len() as jlong,
                    1,
                    addr(out_a.as_mut_ptr()),
                    out_a.len() as jlong,
                    addr(progress_a.as_mut_ptr()),
                ),
                hypertabular_transcode(
                    1,
                    input.as_ptr(),
                    input.len(),
                    1,
                    out_b.as_mut_ptr(),
                    out_b.len(),
                    progress_b.as_mut_ptr().cast::<c_void>(),
                ),
            )
        };
        assert_eq!(x, y);
        assert_eq!(out_a, out_b);
        assert_eq!(progress_a, progress_b);
    }
    // A bad encoding is refused alike.
    let mut progress = [0u64; 4];
    let mut out = [0u8; 4];
    // SAFETY: as above.
    let rc = unsafe {
        native_hypertabular_transcode(
            ENV,
            CLASS_ARG,
            99,
            addr(b"a".as_ptr()),
            1,
            1,
            addr(out.as_mut_ptr()),
            4,
            addr(progress.as_mut_ptr()),
        )
    };
    assert!(rc < 0);
}

#[test]
fn a_delimited_init_with_through_the_native_is_the_exports() {
    // Size, separator ',', quoting 1, skip blanks 0, engine 0, skip 0, header row 0, reject,
    // reject: then the same words with an engine no core has.
    let good: [u8; 24] = [
        24, 0, 0, 0, b',', 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut bad = good;
    bad[7] = 200;
    // SAFETY: takes no arguments.
    let size = unsafe { hypertabular_delimited_state_size() };
    for options in [good, bad] {
        let mut a = state(size);
        let mut b = state(size);
        // SAFETY: each state is the core's size and aligned; the options are its 24 bytes.
        let (x, y) = unsafe {
            (
                native_hypertabular_delimited_init_with(
                    ENV,
                    CLASS_ARG,
                    addr(a.as_mut_ptr()),
                    addr(options.as_ptr()),
                ),
                hypertabular_delimited_init_with(
                    b.as_mut_ptr().cast::<c_void>(),
                    options.as_ptr().cast::<c_void>(),
                ),
            )
        };
        assert_eq!(x, y);
        assert_eq!(a, b);
    }
}

#[test]
fn a_fill_with_with_nothing_to_fill_is_the_exports() {
    let options: [u8; 24] = [
        24, 0, 0, 0, b',', 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    // SAFETY: takes no arguments.
    let size = unsafe { hypertabular_delimited_state_size() };
    let mut a = state(size);
    let mut b = state(size);
    let mut buffers = [0u64; 18];
    buffers[0] = 144;
    let input = b"";
    let mut out_a = [0u64; FILLED_WORDS];
    let mut out_b = [0u64; FILLED_WORDS];
    // SAFETY: every pointer is live memory of the size beside it, none read past a zero length.
    unsafe {
        assert_eq!(
            native_hypertabular_delimited_init_with(
                ENV,
                CLASS_ARG,
                addr(a.as_mut_ptr()),
                addr(options.as_ptr()),
            ),
            hypertabular_delimited_init_with(
                b.as_mut_ptr().cast::<c_void>(),
                options.as_ptr().cast::<c_void>(),
            )
        );
        let x = native_hypertabular_delimited_fill_with(
            ENV,
            CLASS_ARG,
            addr(a.as_mut_ptr()),
            addr(input.as_ptr()),
            0,
            1,
            0,
            0,
            0,
            1,
            addr(buffers.as_ptr()),
            addr(out_a.as_mut_ptr()),
        );
        let y = hypertabular_delimited_fill_with(
            b.as_mut_ptr().cast::<c_void>(),
            input.as_ptr(),
            0,
            1,
            null(),
            null(),
            0,
            1,
            buffers.as_ptr().cast::<c_void>(),
            out_b.as_mut_ptr().cast::<c_void>(),
        );
        assert_eq!(x, y);
    }
    assert_eq!(out_a, out_b);
    assert_eq!(a, b);
}

#[test]
fn faults_with_nothing_to_walk_is_the_exports() {
    let mut cursor_a = 0u64;
    let mut cursor_b = 0u64;
    let mut written_a = 7usize;
    let mut written_b = 7usize;
    let mut out_a = [0u64; 8];
    let mut out_b = [0u64; 8];
    // SAFETY: no column and no row: nothing is read past the live pointers.
    let (x, y) = unsafe {
        (
            native_hypertabular_faults(
                ENV,
                CLASS_ARG,
                0,
                0,
                0,
                0,
                0,
                0,
                addr(&raw mut cursor_a),
                addr(out_a.as_mut_ptr()),
                1,
                addr(&raw mut written_a),
            ),
            hypertabular_faults(
                null(),
                0,
                0,
                null(),
                0,
                0,
                &raw mut cursor_b,
                out_b.as_mut_ptr().cast::<c_void>(),
                1,
                &raw mut written_b,
            ),
        )
    };
    assert_eq!(x, y);
    assert_eq!((written_a, cursor_a), (written_b, cursor_b));
}

#[test]
fn a_suggestion_through_the_native_is_the_exports() {
    // One column of two rows, 1 and 2.5: a suggestion for F64. Spans are {offset, len}.
    let input = b"12.5";
    let cells: [u32; 4] = [0, 1, 1, 3];
    let mut out_a = [0u32; 5];
    let mut out_b = [0u32; 5];
    // SAFETY: two rows of one column over the live cells; the formats are null, as allowed.
    let (x, y) = unsafe {
        (
            native_hypertabular_suggest(
                ENV,
                CLASS_ARG,
                addr(input.as_ptr()),
                input.len() as jlong,
                addr(input.as_ptr()),
                0,
                addr(cells.as_ptr()),
                0,
                2,
                1,
                0,
                addr(out_a.as_mut_ptr()),
            ),
            hypertabular_suggest(
                input.as_ptr(),
                input.len(),
                input.as_ptr(),
                0,
                cells.as_ptr().cast::<c_void>(),
                null(),
                2,
                1,
                null(),
                out_b.as_mut_ptr().cast::<c_void>(),
            ),
        )
    };
    assert_eq!(x, y);
    assert_eq!(x, 0);
    assert_eq!(out_a, out_b);
    assert_eq!(out_a[2], 1, "{out_a:?}");
    assert_eq!((out_a[3], out_a[4]), (2, 2), "{out_a:?}");
}

#[test]
fn the_workbook_with_calls_on_a_non_workbook_are_the_exports() {
    let container = b"not a workbook";
    let options: [u32; 9] = [36, 1, 1, 0, 0, 0, 0, 0, 0];
    let mut buffers = [0u64; 18];
    buffers[0] = 144;
    // SAFETY: takes no arguments.
    let size = unsafe { hypertabular_workbook_state_size() };
    let mut a = state(size);
    let mut b = state(size);
    let mut out_a = [0u64; FILLED_WORDS];
    let mut out_b = [0u64; FILLED_WORDS];
    // SAFETY: each pointer is live memory of the size beside it. An unopened state is what the
    // core refuses, and it does so alike through both.
    unsafe {
        let x = native_hypertabular_workbook_sheet_with(
            ENV,
            CLASS_ARG,
            addr(a.as_mut_ptr()),
            addr(container.as_ptr()),
            container.len() as jlong,
            addr(container.as_ptr()),
            0,
            0,
            addr(options.as_ptr()),
            addr(out_a.as_mut_ptr()),
        );
        let y = hypertabular_workbook_sheet_with(
            b.as_mut_ptr().cast::<c_void>(),
            container.as_ptr(),
            container.len(),
            container.as_ptr(),
            0,
            0,
            options.as_ptr().cast::<c_void>(),
            out_b.as_mut_ptr().cast::<c_void>(),
        );
        assert_eq!(x, y);
        assert_eq!(out_a, out_b);
        let x = native_hypertabular_workbook_fill_with(
            ENV,
            CLASS_ARG,
            addr(a.as_mut_ptr()),
            addr(container.as_ptr()),
            container.len() as jlong,
            0,
            0,
            0,
            1,
            addr(buffers.as_ptr()),
            addr(out_a.as_mut_ptr()),
        );
        let y = hypertabular_workbook_fill_with(
            b.as_mut_ptr().cast::<c_void>(),
            container.as_ptr(),
            container.len(),
            null(),
            null(),
            0,
            1,
            buffers.as_ptr().cast::<c_void>(),
            out_b.as_mut_ptr().cast::<c_void>(),
        );
        assert_eq!(x, y);
        assert_eq!(out_a, out_b);
    }
}

unsafe extern "system" fn find_class(_env: *mut JNIEnv, name: *const c_char) -> jclass {
    LOAD.with(|l| {
        let mut l = l.borrow_mut();
        // SAFETY: the library passes a NUL-terminated name.
        l.found_class = unsafe { core::ffi::CStr::from_ptr(name) }
            .to_str()
            .unwrap()
            .to_owned();
        if l.find_class_fails {
            null_mut()
        } else {
            CLASS_TOKEN as jclass
        }
    })
}

unsafe extern "system" fn register_natives(
    _env: *mut JNIEnv,
    class: jclass,
    methods: *const JNINativeMethod,
    count: jint,
) -> jint {
    assert_eq!(class as usize, CLASS_TOKEN);
    LOAD.with(|l| {
        let mut l = l.borrow_mut();
        for i in 0..count as usize {
            // SAFETY: the library passes `count` entries of NUL-terminated strings.
            let m = unsafe { &*methods.add(i) };
            let text = |p: *mut c_char| {
                // SAFETY: as above.
                unsafe { core::ffi::CStr::from_ptr(p) }
                    .to_str()
                    .unwrap()
                    .to_owned()
            };
            l.registered
                .push((text(m.name), text(m.signature), m.fnPtr as usize));
        }
        if l.register_fails { JNI_ERR } else { JNI_OK }
    })
}

unsafe extern "system" fn get_env(_vm: *mut JavaVM, penv: *mut *mut c_void, version: jint) -> jint {
    assert_eq!(version, JNI_VERSION_1_8);
    if LOAD.with(|l| l.borrow().get_env_fails) {
        return JNI_ERR;
    }
    // SAFETY: the library passes a live out-pointer.
    unsafe { *penv = LOAD.with(|l| l.borrow().env) as *mut c_void };
    JNI_OK
}

/// A stub `JavaVM` whose `GetEnv` hands out a stub `JNIEnv` with the two entries
/// `JNI_OnLoad` calls. Only those entries are written, through their addresses.
fn on_load(load: Load) -> (jint, Load) {
    LOAD.with(|l| *l.borrow_mut() = load);
    let mut iface = Box::new(MaybeUninit::<JNINativeInterface_>::uninit());
    let p = iface.as_mut_ptr();
    // SAFETY: each field is written through its own address.
    unsafe {
        addr_of_mut!((*p).v1_2.FindClass).write(find_class);
        addr_of_mut!((*p).v1_2.RegisterNatives).write(register_natives);
    }
    let mut env_table: Box<*const JNINativeInterface_> = Box::new(p.cast_const());
    LOAD.with(|l| l.borrow_mut().env = &mut *env_table as *mut *const JNINativeInterface_ as usize);
    let mut invoke = Box::new(MaybeUninit::<JNIInvokeInterface_>::uninit());
    let q = invoke.as_mut_ptr();
    // SAFETY: as above.
    unsafe { addr_of_mut!((*q).v1_2.GetEnv).write(get_env) };
    let mut vm_table: Box<*const JNIInvokeInterface_> = Box::new(q.cast_const());
    // SAFETY: the stub VM is live for the call.
    let rc = unsafe {
        JNI_OnLoad(
            &mut *vm_table as *mut *const JNIInvokeInterface_,
            null_mut(),
        )
    };
    drop((iface, invoke, env_table));
    (rc, LOAD.with(|l| core::mem::take(&mut *l.borrow_mut())))
}

#[test]
fn on_load_registers_the_whole_table_on_native() {
    let (rc, load) = on_load(Load::default());
    assert_eq!(rc, JNI_VERSION_1_8);
    assert_eq!(load.found_class, "io/github/skunkwerkx/hypertabular/Native");
    assert_eq!(load.registered.len(), NATIVES.len());
    for (registered, native) in load.registered.iter().zip(NATIVES.iter()) {
        assert_eq!(registered.0, name_of(native));
        assert_eq!(registered.1, desc_of(native));
        assert_eq!(registered.2, native.function as usize);
    }
}

#[test]
fn on_load_fails_without_an_env() {
    let (rc, load) = on_load(Load {
        get_env_fails: true,
        ..Load::default()
    });
    assert_eq!(rc, JNI_ERR);
    assert!(load.found_class.is_empty());
}

#[test]
fn on_load_fails_without_the_class() {
    let (rc, load) = on_load(Load {
        find_class_fails: true,
        ..Load::default()
    });
    assert_eq!(rc, JNI_ERR);
    assert!(load.registered.is_empty());
}

#[test]
fn on_load_fails_when_registration_does() {
    let (rc, load) = on_load(Load {
        register_fails: true,
        ..Load::default()
    });
    assert_eq!(rc, JNI_ERR);
    assert_eq!(load.registered.len(), NATIVES.len());
}

// --- the sources, held to each other ---

fn source(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every JNI entry point carries the no-panic attribute: the example's link-time proof covers
/// only the fns that have it, so one dropped attribute would pass the proof untested.
#[test]
fn every_entry_point_is_under_the_no_panic_proof() {
    let lib = source("src/lib.rs");
    let lines: Vec<&str> = lib.lines().collect();
    let mut entry_points = 0;
    for (number, line) in lines.iter().enumerate() {
        let Some(rest) = line
            .trim_start()
            .strip_prefix("pub unsafe extern \"system\" fn ")
        else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        let mut above = number;
        let mut attributes = Vec::new();
        while above > 0 && lines[above - 1].trim_start().starts_with("#[") {
            above -= 1;
            attributes.push(lines[above]);
        }
        assert!(
            attributes.iter().any(|a| a.contains("no_panic::no_panic")),
            "{name} (src/lib.rs:{}) has no no_panic attribute",
            number + 1
        );
        entry_points += 1;
    }
    // Every native but the five the `book!` macro stamps out, its one body counted once, and
    // JNI_OnLoad.
    assert_eq!(entry_points, NATIVES.len() - 5 + 1 + 1);
    // And the proof calls each one: every native the table registers, and JNI_OnLoad.
    let example = source("examples/no_panic.rs");
    let mut functions: Vec<String> = EXPORTS.iter().map(|(e, _)| format!("native_{e}")).collect();
    functions.push("native_jni_natives".to_owned());
    functions.push("JNI_OnLoad".to_owned());
    for function in functions {
        assert!(
            example.contains(&format!("hypertabular_jni::{function}(")),
            "examples/no_panic.rs does not call {function}"
        );
    }
}

/// The Java declarations are the table: every `private static native` in the JNI `Native`,
/// its descriptor spelled from its Java types, is one registered native, and nothing else is.
#[test]
fn the_java_declarations_are_the_table() {
    let java = source("../src/main/java/io/github/skunkwerkx/hypertabular/Native.java");
    let jni = |ty: &str| match ty {
        "int" => "I",
        "long" => "J",
        other => panic!("a native takes or returns only int and long, not {other}"),
    };
    let mut declared = Vec::new();
    for piece in java.split("private static native ").skip(1) {
        let declaration: String = piece[..piece.find(';').unwrap()]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let (head, params) = declaration.split_once('(').unwrap();
        let (ret, name) = head.trim().split_once(' ').unwrap();
        let params = params.trim_end_matches(')');
        let args: String = params
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| jni(p.split_whitespace().next().unwrap()))
            .collect();
        declared.push((name.to_owned(), format!("({args}){}", jni(ret))));
    }
    let mut table: Vec<(String, String)> = NATIVES
        .iter()
        .map(|n| (name_of(n).to_owned(), desc_of(n).to_owned()))
        .collect();
    // Registration is by name; the order is each file's own.
    declared.sort();
    table.sort();
    assert_eq!(declared, table);
}
