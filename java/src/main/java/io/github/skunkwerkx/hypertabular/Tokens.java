package io.github.skunkwerkx.hypertabular;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Objects;

/**
 * A list of what stands for no value: a cell whose exact, unquoted bytes are one of them is
 * empty, whatever its door, and {@link Batch#origin} says {@link Origin#TOKEN}. A quoted cell
 * never is one. Two lists are equal when they hold the same tokens in the same order.
 */
public final class Tokens {
    /** No tokens. */
    public static final Tokens NONE = new Tokens(new byte[0]);

    /** Each token a little-endian 32-bit length, then its UTF-8 bytes, as the core reads them. */
    private final byte[] packed;

    private Tokens(byte[] packed) {
        this.packed = packed;
    }

    /**
     * The list of these tokens.
     *
     * @param tokens the tokens, matched as their exact UTF-8 bytes
     * @return the list
     * @throws NullPointerException if a token is {@code null}
     */
    public static Tokens of(String... tokens) {
        byte[][] each = new byte[tokens.length][];
        long length = 0;
        for (int index = 0; index < tokens.length; index++) {
            each[index] = Objects.requireNonNull(tokens[index], "token").getBytes(StandardCharsets.UTF_8);
            length += 4L + each[index].length;
        }
        ByteBuffer packed = ByteBuffer.allocate(Math.toIntExact(length)).order(ByteOrder.LITTLE_ENDIAN);
        for (byte[] token : each) {
            packed.putInt(token.length).put(token);
        }
        return new Tokens(packed.array());
    }

    /** The list as the core reads it: the caller does not change it. */
    byte[] packed() {
        return packed;
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Tokens that && Arrays.equals(packed, that.packed);
    }

    @Override
    public int hashCode() {
        return Arrays.hashCode(packed);
    }

    @Override
    public String toString() {
        StringBuilder text = new StringBuilder("Tokens[");
        ByteBuffer rest = ByteBuffer.wrap(packed).order(ByteOrder.LITTLE_ENDIAN);
        while (rest.hasRemaining()) {
            byte[] token = new byte[rest.getInt()];
            rest.get(token);
            text.append('"').append(new String(token, StandardCharsets.UTF_8)).append('"');
            if (rest.hasRemaining()) {
                text.append(", ");
            }
        }
        return text.append(']').toString();
    }
}
