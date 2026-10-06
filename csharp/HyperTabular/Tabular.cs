using HyperCast.Interop;

namespace HyperTabular;

/// <summary>The native core this assembly binds: whether it loaded, and which version answered.</summary>
public static class Tabular
{
	// The one place the binding catches: loading is the caller's environment, not their
	// data, and the point of the probe is to answer "did the native library resolve"
	// without making the first real read the thing that finds out. HyperCast's probe, for
	// this library's version export.
	static readonly Lazy<Version?> _nativeVersion =
		new(() => Abi.ProbeVersion(Native.hypertabular_version), LazyThreadSafetyMode.PublicationOnly);

	/// <summary>
	/// <see langword="true"/> when the native library resolved and answered the version
	/// probe. Probed once, then cached; a <see langword="false"/> is permanent for the
	/// process. Gate on this instead of catching <see cref="DllNotFoundException"/> around
	/// the first read.
	/// </summary>
	public static bool IsAvailable => _nativeVersion.Value is not null;

	/// <summary>
	/// The native core's own version — <c>major.minor.patch</c> as the library reports it,
	/// or <see langword="null"/> when it did not load. Compare against this assembly's
	/// version to name a mismatch before the first read.
	/// </summary>
	public static Version? NativeVersion => _nativeVersion.Value;
}
