//! Files and streams, through PHP's own functions, as 1.0 read them: `fopen`, `fread`, `feof`,
//! `fclose`, `stream_get_contents` and `file_get_contents`, so every stream wrapper, context and
//! `open_basedir` rule applies exactly as it does to PHP code. A warning 1.0 silenced with `@`
//! is silenced the same way here — `error_reporting(0)` for the call, which is what `@` does —
//! and still reaches `error_get_last()`, where the message 1.0 quoted comes from.

use ext_php_rs::exception::PhpResult;
use ext_php_rs::types::Zval;

use super::classes::{bytes_zval, call, int_zval};
use super::errors::{invalid_argument, runtime};

/// 1.0's refusal of anything but an open stream.
const NOT_A_STREAM: &str = "The stream must be an open stream resource";

/// Refuses a value that is not an open stream resource, as 1.0's
/// `!\is_resource($stream) || get_resource_type($stream) !== 'stream'` did.
pub(crate) fn check_stream(stream: &Zval) -> PhpResult<()> {
    if !stream.is_resource() || !call(None, "is_resource", &mut [stream.shallow_clone()])?.is_true()
    {
        return Err(invalid_argument(NOT_A_STREAM));
    }
    let kind = call(None, "get_resource_type", &mut [stream.shallow_clone()])?;
    if kind.zend_str().map(|s| s.as_bytes()) != Some(b"stream") {
        return Err(invalid_argument(NOT_A_STREAM));
    }
    Ok(())
}

/// Calls `name` with warnings silenced, as `@name(...)` does; `None` for `false`.
fn quietly(name: &str, args: &mut [Zval]) -> PhpResult<Option<Zval>> {
    let previous = call(None, "error_reporting", &mut [int_zval(0)])?;
    let result = call(None, name, args);
    // Restored whatever the call did; with an exception pending this is skipped by the engine,
    // as a `finally` would not be, so restore it before looking at the result.
    let restored = call(None, "error_reporting", &mut [previous]);
    let value = result?;
    restored?;
    Ok((!value.is_false()).then_some(value))
}

/// 1.0's `cannot open` error: the path and what PHP last said about it.
fn cannot_open(path: &[u8]) -> PhpResult<ext_php_rs::exception::PhpException> {
    let last = call(None, "error_get_last", &mut [])?;
    let reason = last
        .array()
        .and_then(|error| error.get("message"))
        .and_then(Zval::zend_str)
        .map_or_else(
            || "unknown error".to_owned(),
            |s| String::from_utf8_lossy(s.as_bytes()).into_owned(),
        );
    Ok(runtime(format!(
        "hypertabular: cannot open {}: {reason}",
        String::from_utf8_lossy(path)
    )))
}

/// `@fopen($path, 'rb')`, or 1.0's error.
pub(crate) fn open(path: &[u8]) -> PhpResult<Zval> {
    match quietly("fopen", &mut [bytes_zval(path), bytes_zval(b"rb")])? {
        Some(stream) => Ok(stream),
        None => Err(cannot_open(path)?),
    }
}

/// `@file_get_contents($path)`, or 1.0's error.
pub(crate) fn contents(path: &[u8]) -> PhpResult<Vec<u8>> {
    match quietly("file_get_contents", &mut [bytes_zval(path)])? {
        Some(bytes) => Ok(bytes
            .zend_str()
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default()),
        None => Err(cannot_open(path)?),
    }
}

/// `fread($stream, $length)`; `None` for `false`.
pub(crate) fn read(stream: &Zval, length: usize) -> PhpResult<Option<Vec<u8>>> {
    let length = i64::try_from(length).unwrap_or(i64::MAX);
    let chunk = call(
        None,
        "fread",
        &mut [stream.shallow_clone(), int_zval(length)],
    )?;
    Ok(chunk.zend_str().map(|s| s.as_bytes().to_vec()))
}

/// `feof($stream)`.
pub(crate) fn at_end(stream: &Zval) -> PhpResult<bool> {
    Ok(call(None, "feof", &mut [stream.shallow_clone()])?.is_true())
}

/// `stream_get_contents($stream)`; `None` for `false`.
pub(crate) fn rest(stream: &Zval) -> PhpResult<Option<Vec<u8>>> {
    let bytes = call(None, "stream_get_contents", &mut [stream.shallow_clone()])?;
    Ok(bytes.zend_str().map(|s| s.as_bytes().to_vec()))
}

/// `if (\is_resource($stream)) fclose($stream);`
pub(crate) fn close(stream: &Zval) -> PhpResult<()> {
    if call(None, "is_resource", &mut [stream.shallow_clone()])?.is_true() {
        call(None, "fclose", &mut [stream.shallow_clone()])?;
    }
    Ok(())
}
