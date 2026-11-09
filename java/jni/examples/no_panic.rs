#![deny(missing_docs)]
//! The link-time proof behind the `no-panic` feature. `#[no_panic]` only reports a function
//! when a release-optimized binary that calls it is linked, and a cdylib does not count: it
//! drops every function no export reaches. So this calls each annotated native once, with
//! inputs the optimizer cannot see through, and building it is the whole check. It needs
//! unwind, because under abort a panic path is invisible to `no-panic`:
//!
//!     CARGO_PROFILE_RELEASE_PANIC=unwind cargo build --release --locked --example no_panic --features no-panic
//!
//! A panic path left in any of them fails the link with `ERROR[no-panic]` naming the
//! function. Running the binary proves nothing: the calls sit behind a condition that is
//! never true.

use core::hint::black_box;
use core::ptr::null_mut;

fn main() {
    if std::env::args().len() > 1000 {
        let env = black_box(null_mut());
        let class = black_box(null_mut());
        let a = black_box(0_i64);
        let n = black_box(0_i32);
        unsafe {
            black_box(hypertabular_jni::native_hypertabular_version(env, class));
            black_box(hypertabular_jni::native_hypertabular_delimited_state_size(
                env, class,
            ));
            black_box(hypertabular_jni::native_hypertabular_delimited_init(
                env, class, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_delimited_header(
                env, class, a, a, a, n, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_delimited_fill(
                env, class, a, a, a, n, a, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_delimited_unescape(
                env, class, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_state_size(
                env, class,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_open(
                env, class, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_sheets(
                env, class, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_strings(
                env, class, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_styles(
                env, class, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_sheet(
                env, class, a, a, a, a, a, n, n, n, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_header(
                env, class, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_fill(
                env, class, a, a, a, a, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_delimited_init_with(
                env, class, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_delimited_fill_with(
                env, class, a, a, a, n, a, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_transcode(
                env, class, n, a, a, n, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_faults(
                env, class, a, a, a, a, n, n, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_suggest(
                env, class, a, a, a, a, a, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_sheet_with(
                env, class, a, a, a, a, a, n, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_workbook_fill_with(
                env, class, a, a, a, a, a, a, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_range_parse(
                env, class, a, a, a,
            ));
            black_box(hypertabular_jni::native_hypertabular_batch_check(
                env, class, a, a, a, a, n, a,
            ));
            black_box(hypertabular_jni::native_jni_natives(env, class));
            black_box(hypertabular_jni::JNI_OnLoad(
                black_box(null_mut()),
                black_box(null_mut()),
            ));
        }
    }
}
