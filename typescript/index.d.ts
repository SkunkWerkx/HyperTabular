import type {
  Broken,
  Dialect as BindingDialect,
  Batch as NativeBatch,
  Row,
  Success,
} from './binding'
export {
  nativeVersion,
  Column,
  Header,
  Range,
  DelimitedReader,
  Workbook,
  Sheet,
  Batch,
  Row,
} from './binding'
export type {
  Outcome,
  Broken,
  TabularError,
  ErrorKind,
  StructuralFailure,
  FailureKind,
  Door,
  Value,
  WorkbookFormat,
  SheetInfo,
  SheetOptions,
  DelimitedOptions,
  PackedNumFormat,
  DeclaredDateOptions,
  DeclaredConstraints,
  ShortRows,
  LongRows,
  Encoding,
  Origin,
  Detail,
  Problem,
  SuggestOptions,
  Suggestion,
  SuggestedColumn,
  SuggestedFormat,
  SuggestStatus,
  Candidate,
  CandidateKind,
} from './binding'
// A cell's verdict and its values are HyperCast's own types, from the peer dependency.
export type {
  Verdict,
  Success,
  Failure,
  CheckedVerdict,
  Checked,
  Violated,
  Violation,
  Fault,
  CastFailure,
  Timestamp,
  CalendarDate,
  TimeOfDay,
  CivilDateTime,
  Duration,
} from '@skunkwerkx/hypercast'

/**
 * How delimited text is written. Every field left out is the core's default: comma-
 * separated, `"` quoting, a header, blank lines skipped, nothing skipped, short and long rows
 * refused, UTF-8, no tokens.
 */
export interface Dialect extends BindingDialect {}
type Named = Readonly<Pick<Required<Dialect>, 'separator' | 'quoting' | 'hasHeader' | 'skipBlankLines'>>
/**
 * The three dialects by name: comma, tab and pipe, each quoted, with a header, blank lines
 * skipped.
 */
export declare const Dialect: {
  readonly CSV: Named
  readonly TSV: Named
  readonly PSV: Named
}

declare module './binding' {
  interface DelimitedReader {
    /** Each read's outcome, batch after batch; an error that stops the read comes last. */
    batches(): Generator<Success<NativeBatch> | Broken, void, undefined>
    /** Every row, batch after batch; an error that stops the read comes last. */
    rows(): Generator<Success<Row> | Broken, void, undefined>
    [Symbol.iterator](): Generator<Success<NativeBatch> | Broken, void, undefined>
  }
  interface Sheet {
    /** Each read's outcome, batch after batch; an error that stops the read comes last. */
    batches(): Generator<Success<NativeBatch> | Broken, void, undefined>
    /** Every row, batch after batch; an error that stops the read comes last. */
    rows(): Generator<Success<Row> | Broken, void, undefined>
    [Symbol.iterator](): Generator<Success<NativeBatch> | Broken, void, undefined>
  }
  interface Batch {
    /** The batch's rows, in order. */
    [Symbol.iterator](): Generator<Row, void, undefined>
  }
}

/** Whether the native addon or its WebAssembly fallback loaded. */
export function isAvailable(): boolean
export type HyperTabularErrorCode = 'ERR_HYPERTABULAR_UNAVAILABLE' | 'ERR_HYPERTABULAR_BUSY'
/**
 * The two errors this package throws: no addon loaded, so no reader can answer
 * (`ERR_HYPERTABULAR_UNAVAILABLE`); and a reader's or sheet's getter read from inside one of
 * its own calls (`ERR_HYPERTABULAR_BUSY`, `kind` `busy`), which has no outcome to answer in.
 */
export interface HyperTabularError extends Error {
  code: HyperTabularErrorCode
  /** `busy` for `ERR_HYPERTABULAR_BUSY`. */
  kind?: 'busy'
}
export function isHyperTabularError(value: unknown): value is HyperTabularError
