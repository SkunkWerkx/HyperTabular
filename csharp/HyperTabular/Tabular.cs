namespace HyperTabular;

/// <summary>The native core this assembly binds: whether it loaded, and which version answered.</summary>
public static class Tabular
{
	static readonly Lazy<Version?> _nativeVersion = new(ProbeNativeVersion, LazyThreadSafetyMode.PublicationOnly);

	// The one place the binding catches: loading is the caller's environment, not their
	// data, and the point of the probe is to answer "did the native library resolve"
	// without making the first real read the thing that finds out.
	static Version? ProbeNativeVersion()
	{
		try
		{
			var packed = Native.hypertabular_version();
			return new Version((int)(packed >> 16), (int)((packed >> 8) & 0xFF), (int)(packed & 0xFF));
		}
		catch (Exception e) when (e is DllNotFoundException or EntryPointNotFoundException
			or BadImageFormatException or PlatformNotSupportedException or TypeInitializationException)
		{
			return null;
		}
	}

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
