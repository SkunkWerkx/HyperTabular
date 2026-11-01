import type { Broken, Batch as NativeBatch, Row, Success } from './binding'
export {
  nativeVersion,
  Column,
  Header,
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
} from './binding'
// A cell's verdict and its values are HyperCast's own types, from the peer dependency.
export type {
  Verdict,
  Success,
  Failure,
  Fault,
  CastFailure,
  Timestamp,
  CalendarDate,
  TimeOfDay,
  CivilDateTime,
  Duration,
} from '@skunkwerkx/hypercast'

/**
 * How delimited text is written: the separator (a tab, or printable ASCII other than `"`),
 * and whether `"` quotes, the first record is the header and blank lines are skipped, each
 * `true` when left out.
 */
export interface Dialect {
  separator: string
  quoting?: boolean
  hasHeader?: boolean
  skipBlankLines?: boolean
}
/** The three dialects by name: comma, tab and pipe, each quoted, with a header. */
export declare const Dialect: {
  readonly CSV: Readonly<Required<Dialect>>
  readonly TSV: Readonly<Required<Dialect>>
  readonly PSV: Readonly<Required<Dialect>>
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
export type HyperTabularErrorCode = 'ERR_HYPERTABULAR_UNAVAILABLE'
/** The one error this package throws: no addon loaded, so no reader can answer. */
export interface HyperTabularError extends Error {
  code: HyperTabularErrorCode
}
export function isHyperTabularError(value: unknown): value is HyperTabularError
