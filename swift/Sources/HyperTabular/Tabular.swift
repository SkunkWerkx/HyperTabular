// HyperCast is the judge, and its types are this module's vocabulary: `Verdict`, `Fault`,
// `CastFailure`, `NumFormat`, `NumStyles`, `DateOrder`, `UnixPrecision` and `ExcelEpoch`
// appear in every signature here and are HyperCast's own, from its package. Re-exported so
// that `import HyperTabular` is enough to name them.
@_exported import HyperCast
import HyperTabularCore

/// The native core this module binds: whether it is there, and which version answered.
public enum Tabular {
    /// Whether the native core is usable. Always `true`: the core is a static library
    /// linked into the executable on every platform this package builds for, and a platform
    /// with no prebuilt core fails to compile rather than at run time. Here so that code
    /// written against the bindings that load a shared library reads the same.
    public static var isAvailable: Bool { true }

    /// The version of the native `libhypertabular` linked into this executable, as
    /// `major.minor.patch` — the library's own answer (`hypertabular_version`), not this
    /// package's tag — so a caller can prove the two agree before the first read.
    public static func nativeVersion() -> String {
        let packed = hypertabular_version()
        return "\(packed >> 16).\(packed >> 8 & 0xFF).\(packed & 0xFF)"
    }
}
