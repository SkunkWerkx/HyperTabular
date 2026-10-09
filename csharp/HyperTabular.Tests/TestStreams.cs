namespace HyperTabular.Tests;

/// <summary>
/// A forward-only stream over bytes that hands out a few bytes a read — 1 to
/// <c>maxChunk</c>, varying — so that a reader of it has to refill many times, and that says
/// whether, and how, it was disposed. Its asynchronous reads really are asynchronous now and
/// then, and can cancel a token after a given number of reads.
/// </summary>
sealed class ShortReadStream(byte[] bytes, int maxChunk = 5) : Stream
{
	int _position;
	int _reads;

	/// <summary>Cancelled, if set, by the read numbered <see cref="CancelAfterReads"/>.</summary>
	public CancellationTokenSource? Cancel { get; set; }

	/// <summary>How many reads go by before <see cref="Cancel"/> is cancelled.</summary>
	public int CancelAfterReads { get; set; } = int.MaxValue;

	/// <summary>
	/// Whether an asynchronous read now and then really is asynchronous. Off, every one
	/// completes synchronously, and a test that sets thread-static switches stays on its thread.
	/// </summary>
	public bool Yields { get; init; } = true;

	public bool Disposed { get; private set; }
	public bool DisposedAsynchronously { get; private set; }
	public int Reads => _reads;

	public override bool CanRead => !Disposed;
	public override bool CanSeek => false;
	public override bool CanWrite => false;
	public override long Length => throw new NotSupportedException();
	public override long Position
	{
		get => throw new NotSupportedException();
		set => throw new NotSupportedException();
	}

	int Next(Span<byte> destination)
	{
		ObjectDisposedException.ThrowIf(Disposed, this);
		var chunk = Math.Min(Math.Min(destination.Length, 1 + _reads++ % maxChunk), bytes.Length - _position);
		bytes.AsSpan(_position, chunk).CopyTo(destination);
		_position += chunk;
		return chunk;
	}

	public override int Read(byte[] buffer, int offset, int count) => Next(buffer.AsSpan(offset, count));

	public override int Read(Span<byte> buffer) => Next(buffer);

	public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
	{
		if (Yields && _reads % 3 == 0)
			await Task.Yield();
		if (_reads >= CancelAfterReads)
			await Cancel!.CancelAsync();
		cancellationToken.ThrowIfCancellationRequested();
		return Next(buffer.Span);
	}

	public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) =>
		ReadAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();

	public override void Flush()
	{
	}

	public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
	public override void SetLength(long value) => throw new NotSupportedException();
	public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

	protected override void Dispose(bool disposing)
	{
		Disposed = true;
		base.Dispose(disposing);
	}

	public override ValueTask DisposeAsync()
	{
		DisposedAsynchronously = true;
		Disposed = true;
		return default;
	}
}

/// <summary>A seekable stream that claims more bytes than an array can hold, and fails any read: what is refused must be refused before it is read.</summary>
sealed class HugeStream : Stream
{
	public override bool CanRead => true;
	public override bool CanSeek => true;
	public override bool CanWrite => false;
	public override long Length => (long)int.MaxValue + 1;
	public override long Position { get; set; }
	public override void Flush()
	{
	}
	public override int Read(byte[] buffer, int offset, int count) => throw new InvalidOperationException("read");
	public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
	public override void SetLength(long value) => throw new NotSupportedException();
	public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
}
