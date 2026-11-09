using System.Text;

namespace HyperTabular.Tests;

/// <summary>
/// Caller code in the middle of a read: the stream a reader reads runs between the reader's own
/// steps, and anything it calls back into that reader is refused, never answered from buffers the
/// read has offsets into. A second read while a <see cref="DelimitedReader.ReadAsync"/> waits is
/// the same.
/// </summary>
public sealed class ReentrancyTests
{
	const string Busy = "middle of a read";

	static readonly Column[] _plan = [Column.Text(0), Column.Text(1)];

	public static TheoryData<TextEncoding> Encodings => [TextEncoding.Utf8, TextEncoding.Latin1, TextEncoding.Utf16Le];

	/// <summary>A header and <paramref name="count"/> rows of an id and a name of seeded length, an <c>é</c> every seventh, in <paramref name="encoding"/>; and the rows a read should give.</summary>
	static (byte[] Bytes, List<(int Line, string? Id, string? Name)> Rows) Data(int count, int seed, TextEncoding encoding)
	{
		var random = new Random(seed);
		var text = new StringBuilder("id,name\n");
		var rows = new List<(int, string?, string?)>();
		for (var index = 0; index < count; index++)
		{
			var name = "n" + index + new string((char)('a' + index % 26), random.Next(0, 40)) + (index % 7 == 0 ? "é" : "");
			text.Append(index).Append(',').Append(name).Append('\n');
			rows.Add((index + 2, index.ToString(System.Globalization.CultureInfo.InvariantCulture), name));
		}
		var bytes = encoding switch
		{
			TextEncoding.Utf16Le => Encoding.Unicode.GetBytes(text.ToString()),
			TextEncoding.Latin1 => Encoding.Latin1.GetBytes(text.ToString()),
			_ => Encoding.UTF8.GetBytes(text.ToString()),
		};
		return (bytes, rows);
	}

	static void Take(Batch? batch, List<(int, string?, string?)> into)
	{
		for (var row = 0; batch is not null && row < batch.Rows; row++)
			into.Add((batch.Line(row), batch.GetString(0, row), batch.GetString(1, row)));
	}

	static List<(int, string?, string?)> ReadAll(DelimitedReader reader, List<(int, string?, string?)> rows)
	{
		for (Batch? batch; (batch = reader.Read()) is not null;)
			Take(batch, rows);
		return rows;
	}

	[Theory]
	[MemberData(nameof(Encodings))]
	void A_call_from_the_stream_back_into_its_reader_is_refused_and_the_read_goes_on_whole(TextEncoding encoding)
	{
		for (var seed = 0; seed < 20; seed++)
		{
			var (bytes, want) = Data(300, seed, encoding);
			var stream = new HookStream(bytes, seed);
			DelimitedReader? reader = null;
			var refused = 0;
			var inside = false;
			// Every read of the stream once the reader exists, from the first refill to the last:
			// with a 16-byte buffer and seven-row batches that is mid-record, mid-batch, while the
			// buffer grows and, under an encoding, mid-character.
			stream.Hook = call =>
			{
				// Once per read of the stream: a call that got through would read the stream again.
				if (reader is null || inside)
					return;
				inside = true;
				Should.Throw<InvalidOperationException>(() => reader.Read()).Message.ShouldContain(Busy);
				Should.Throw<InvalidOperationException>(() => reader.ReadAsync().AsTask().GetAwaiter().GetResult())
					.Message.ShouldContain(Busy);
				Should.Throw<InvalidOperationException>(() => reader.Bind(_plan)).Message.ShouldContain(Busy);
				Should.Throw<InvalidOperationException>(() =>
				{
					foreach (var row in reader.Rows())
						_ = row.Line;
				}).Message.ShouldContain(Busy);
				refused++;
				inside = false;
			};
			reader = new DelimitedReader(stream, Dialect.Csv with { Encoding = encoding }, _plan, batchRows: 7, bufferBytes: 16);
			ReadAll(reader, []).ShouldBe(want, $"seed {seed}");
			refused.ShouldBeGreaterThan(10, $"seed {seed}");
			reader.Dispose();
		}
	}

	[Theory]
	[MemberData(nameof(Encodings))]
	void A_dispose_from_the_stream_ends_that_read_and_every_later_one(TextEncoding encoding)
	{
		// Every dispose point from the first refill on, over seeds that move the record and character
		// boundaries, by Dispose and DisposeAsync, and with a stream that goes on giving bytes or says
		// it has ended once the reader is gone: the read the dispose fell in is the one that throws.
		foreach (var point in new[] { 1, 2, 3, 5, 9, 17, 31, 60 })
		{
			for (var seed = 0; seed < 4; seed++)
			{
				foreach (var ends in new[] { false, true })
				{
					var asynchronously = (point + seed) % 2 == 1;
					var (bytes, want) = Data(300, seed, encoding);
					var stream = new HookStream(bytes, seed);
					DelimitedReader? reader = null;
					var reads = 0;
					var fired = 0;
					stream.Hook = call =>
					{
						if (fired != 0 || call < point || reader is null)
							return;
						fired = reads;
						if (asynchronously)
							reader.DisposeAsync().AsTask().GetAwaiter().GetResult();
						else
							reader.Dispose();
						stream.Ended = ends;
					};
					reader = new DelimitedReader(stream, Dialect.Csv with { Encoding = encoding }, _plan, batchRows: 7, bufferBytes: 16);
					var got = new List<(int, string?, string?)>();
					Batch? kept = null;
					var context = $"point {point}, seed {seed}, ends {ends}, async {asynchronously}";
					var ended = Should.Throw<ObjectDisposedException>(() =>
					{
						for (; ; )
						{
							reads++;
							var batch = reader.Read();
							if (batch is null)
								break;
							fired.ShouldBe(0, $"no batch comes out of the read the dispose fell in ({context})");
							kept = batch;
							Take(batch, got);
						}
					}, context);
					ended.Message.ShouldContain("disposed in the middle of a read", customMessage: context);
					fired.ShouldBe(reads, $"the read the dispose fell in is the one that throws ({context})");
					got.ShouldBe(want.Take(got.Count), $"what came before the dispose is a correct prefix ({context})");
					(kept?.Rows ?? 0).ShouldBe(0, $"the batch handed out before is over ({context})");
					Should.Throw<ObjectDisposedException>(() => reader.Read());
					Should.Throw<ObjectDisposedException>(() => reader.ReadAsync().AsTask().GetAwaiter().GetResult());
					Should.Throw<ObjectDisposedException>(() =>
					{
						foreach (var row in reader.Rows())
							_ = row.Line;
					});
				}
			}
		}
	}

	[Fact]
	async Task A_second_read_while_a_ReadAsync_waits_on_the_stream_is_refused()
	{
		var (bytes, want) = Data(200, 3, TextEncoding.Utf8);
		var stream = new HookStream(bytes, 3, maxChunk: 23);
		using var reader = new DelimitedReader(stream, Dialect.Csv, _plan, batchRows: 5, bufferBytes: 16);
		var got = new List<(int, string?, string?)>();
		Take(reader.Read(), got);

		var hold = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
		stream.Hold = hold;
		var pending = reader.ReadAsync(TestContext.Current.CancellationToken);
		pending.IsCompleted.ShouldBeFalse();
		Should.Throw<InvalidOperationException>(() => reader.Read()).Message.ShouldContain(Busy);
		(await Should.ThrowAsync<InvalidOperationException>(reader.ReadAsync(TestContext.Current.CancellationToken).AsTask()))
			.Message.ShouldContain(Busy);
		Should.Throw<InvalidOperationException>(() => reader.Bind(_plan)).Message.ShouldContain(Busy);

		hold.SetResult();
		Take(await pending, got);
		// Until the pending read completed the reader was busy; from here it reads on, in either form.
		Take(await reader.ReadAsync(TestContext.Current.CancellationToken), got);
		ReadAll(reader, got).ShouldBe(want);
	}

	[Fact]
	async Task A_dispose_while_a_ReadAsync_waits_ends_that_read_with_no_batch()
	{
		var (bytes, _) = Data(200, 4, TextEncoding.Utf8);
		var stream = new HookStream(bytes, 4, maxChunk: 23);
		var reader = new DelimitedReader(stream, Dialect.Csv, _plan, batchRows: 5, bufferBytes: 16, leaveOpen: true);
		reader.Read().ShouldNotBeNull();

		var hold = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
		stream.Hold = hold;
		var pending = reader.ReadAsync(TestContext.Current.CancellationToken);
		pending.IsCompleted.ShouldBeFalse();
		reader.Dispose();
		hold.SetResult();
		(await Should.ThrowAsync<ObjectDisposedException>(pending.AsTask())).Message.ShouldContain("disposed in the middle of a read");
		Should.Throw<ObjectDisposedException>(() => reader.Read());
	}

	[Theory]
	[MemberData(nameof(Encodings))]
	async Task A_read_that_fails_leaves_the_reader_free_for_the_next(TextEncoding encoding)
	{
		var (bytes, want) = Data(300, 5, encoding);
		var stream = new HookStream(bytes, 5);
		var opened = false;
		stream.Hook = call =>
		{
			if (opened && call % 6 == 0)
				throw new IOException("the stream failed");
		};
		using var reader = new DelimitedReader(stream, Dialect.Csv with { Encoding = encoding }, _plan, batchRows: 7, bufferBytes: 16);
		opened = true;
		var got = new List<(int, string?, string?)>();
		var failures = 0;
		for (var done = false; !done;)
		{
			try
			{
				var batch = encoding == TextEncoding.Latin1
					? await reader.ReadAsync(TestContext.Current.CancellationToken)
					: reader.Read();
				done = batch is null;
				Take(batch, got);
			}
			catch (IOException)
			{
				failures++;
			}
		}
		failures.ShouldBeGreaterThan(0);
		got.ShouldBe(want);
	}

	[Fact]
	async Task A_ReadAsync_refused_before_it_waits_leaves_the_reader_free()
	{
		var (bytes, want) = Data(50, 7, TextEncoding.Utf8);
		using var reader = new DelimitedReader(new HookStream(bytes, 7), Dialect.Csv);
		await Should.ThrowAsync<InvalidOperationException>(reader.ReadAsync(TestContext.Current.CancellationToken).AsTask());
		reader.Bind(_plan, 7);
		ReadAll(reader, []).ShouldBe(want);
	}

	[Fact]
	async Task A_cancelled_ReadAsync_leaves_the_reader_free_for_the_next()
	{
		var (bytes, want) = Data(200, 6, TextEncoding.Utf8);
		var stream = new HookStream(bytes, 6, maxChunk: 23);
		using var reader = new DelimitedReader(stream, Dialect.Csv, _plan, batchRows: 5, bufferBytes: 16);
		var got = new List<(int, string?, string?)>();
		Take(reader.Read(), got);
		using var cancel = new CancellationTokenSource();
		var hold = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
		stream.Hold = hold;
		var pending = reader.ReadAsync(cancel.Token);
		await cancel.CancelAsync();
		hold.SetResult();
		await Should.ThrowAsync<OperationCanceledException>(pending.AsTask());
		ReadAll(reader, got).ShouldBe(want);
	}

	/// <summary>A source whose first read is a reader's, and whose next fails as told: a failure just past a full sample.</summary>
	sealed class FailingPastTheSample(DelimitedReader reader, Exception failure) : IBatchSource
	{
		int _reads;

		public Batch? Read() => _reads++ == 0 ? reader.Read() : throw failure;
	}

	[Fact]
	void Past_a_full_sample_only_an_IO_failure_says_truncated_and_a_cancellation_is_thrown()
	{
		var text = Encoding.UTF8.GetBytes("a,b\n1,x\n2,y\n3,z\n4,w\n");
		Sample Of(Exception failure)
		{
			using var reader = new DelimitedReader(text.AsMemory(), Dialect.Csv);
			return Sample.Of(new FailingPastTheSample(reader, failure), plan => reader.Bind(plan, 3), 2, 3, sheet: false);
		}

		Of(new IOException("the disk failed")).Judge(null, SuggestOptions.Default).Truncated.ShouldBeTrue();
		Should.Throw<OperationCanceledException>(() => Of(new OperationCanceledException()));
		Should.Throw<ThreadInterruptedException>(() => Of(new ThreadInterruptedException()));
		Should.Throw<InvalidOperationException>(() => Of(new InvalidOperationException("the caller's own")));
	}

	[Fact]
	void A_read_inside_a_loop_over_Rows_makes_the_loop_refuse_to_go_on()
	{
		var text = Encoding.UTF8.GetBytes("id,name\n1,a\n2,b\n3,c\n4,d\n5,e\n6,f\n");
		using var reader = new DelimitedReader(text.AsMemory(), Dialect.Csv, _plan, batchRows: 2);
		var lines = new List<int>();
		Should.Throw<InvalidOperationException>(() =>
		{
			foreach (var row in reader.Rows())
			{
				lines.Add(row.Line);
				if (row.Line == 2)
					reader.Read();
			}
		}).Message.ShouldContain("read on");
		lines.ShouldBe([2], "the loop never goes on in a batch it did not read");

		// Read on to a shorter batch inside the loop: the loop must not take its end for its own.
		using var shorter = new DelimitedReader(Encoding.UTF8.GetBytes("id,name\n1,a\n2,b\n3,c\n4,d\n5,e\n").AsMemory(), Dialect.Csv, _plan, batchRows: 2);
		lines.Clear();
		Should.Throw<InvalidOperationException>(() =>
		{
			foreach (var row in shorter.Rows())
			{
				lines.Add(row.Line);
				if (row.Line == 3)
				{
					shorter.Read();
					shorter.Read();
				}
			}
		}).Message.ShouldContain("read on");
		lines.ShouldBe([2, 3]);

		// The loop's own reads are its own: run to the end, it sees every row once.
		using var whole = new DelimitedReader(text.AsMemory(), Dialect.Csv, _plan, batchRows: 2);
		var all = new List<int>();
		foreach (var row in whole.Rows())
			all.Add(row.Line);
		all.ShouldBe([2, 3, 4, 5, 6, 7]);
	}
}
