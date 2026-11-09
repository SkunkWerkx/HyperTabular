package hypertabular

import (
	"errors"
	"fmt"
)

// Header is a header record's names, in column order: what DelimitedReader.Header and
// Sheet.Header return. It is a []string underneath, so it is ranged over, indexed, measured
// and handed to anything that takes a []string as before; what it adds is the lookup a plan
// is built from — the ordinal of a column by its name.
//
// Do not modify it: it is the reader's own.
type Header []string

// ErrNoColumn is the error Header.Ordinal returns, inside a *NoColumnError, for a name the
// header does not have; reach it with errors.Is.
var ErrNoColumn = errors.New("hypertabular: no such column")

// NoColumnError is the error Header.Ordinal returns for a name the header does not have:
// it says which. errors.Is matches it to ErrNoColumn.
type NoColumnError struct {
	// Name is the name that was looked for.
	Name string
}

// Error says which column was not found.
func (e *NoColumnError) Error() string {
	return fmt.Sprintf("hypertabular: the header has no column named %q", e.Name)
}

// Is reports whether target is ErrNoColumn, for errors.Is.
func (e *NoColumnError) Is(target error) bool { return target == ErrNoColumn }

// ErrAmbiguousColumn is the error Header.Ordinal returns, inside an *AmbiguousColumnError, for a
// name two columns of the header share; reach it with errors.Is.
var ErrAmbiguousColumn = errors.New("hypertabular: two columns have that name")

// AmbiguousColumnError is the error Header.Ordinal returns for a name two columns of the header
// share: it won't choose between them. Header.Find takes the first, and a plan can name either by
// position. errors.Is matches it to ErrAmbiguousColumn.
type AmbiguousColumnError struct {
	// Name is the name that was looked for.
	Name string
	// First is the zero-based column that has it first, counted as a plan's ordinals are.
	First int
	// Second is the zero-based column that has it next. A later one isn't named.
	Second int
}

// Error says which columns share the name.
func (e *AmbiguousColumnError) Error() string {
	return fmt.Sprintf("hypertabular: the header names two columns %q, at positions %d and %d: say which by position",
		e.Name, e.First, e.Second)
}

// Is reports whether target is ErrAmbiguousColumn, for errors.Is.
func (e *AmbiguousColumnError) Is(target error) bool { return target == ErrAmbiguousColumn }

// Find is the ordinal of the first column named name, and whether there is one. The match is
// exact — byte for byte, case and spaces included, nothing trimmed or folded — and the first
// of two columns with the same name is the one found, with nothing said of the other: Ordinal
// refuses to choose.
func (h Header) Find(name string) (int, bool) {
	for ordinal, candidate := range h {
		if candidate == name {
			return ordinal, true
		}
	}
	return -1, false
}

// FindBytes is Find for a name held as UTF-8 bytes, the first of two columns with the same name
// included; it allocates nothing.
func (h Header) FindBytes(name []byte) (int, bool) {
	for ordinal, candidate := range h {
		if candidate == string(name) {
			return ordinal, true
		}
	}
	return -1, false
}

// Ordinal is the ordinal of the one column named name, matched as Find matches it — what a
// plan built from names reads best with:
//
//	id, err := header.Ordinal("id")
//
// A name the header does not have is a *NoColumnError that names it, and errors.Is matches it to
// ErrNoColumn. A name two columns share is an *AmbiguousColumnError that names both, and errors.Is
// matches it to ErrAmbiguousColumn: a plan built from names doesn't get to read the wrong one.
func (h Header) Ordinal(name string) (int, error) {
	first, ok := h.Find(name)
	if !ok {
		return -1, &NoColumnError{Name: name}
	}
	for second := first + 1; second < len(h); second++ {
		if h[second] == name {
			return -1, &AmbiguousColumnError{Name: name, First: first, Second: second}
		}
	}
	return first, nil
}

// OrdinalBytes is Ordinal for a name held as UTF-8 bytes.
func (h Header) OrdinalBytes(name []byte) (int, error) {
	first, ok := h.FindBytes(name)
	if !ok {
		return -1, &NoColumnError{Name: string(name)}
	}
	for second := first + 1; second < len(h); second++ {
		if h[second] == string(name) {
			return -1, &AmbiguousColumnError{Name: string(name), First: first, Second: second}
		}
	}
	return first, nil
}

// ErrUnbound is the error Read returns from a DelimitedReader or a Sheet opened without a
// plan, until Bind has given it one. It is not the reader's last word: once a plan is
// bound, Read reads.
var ErrUnbound = errors.New("hypertabular: no plan is bound; call Bind before Read")

// ErrAlreadyBound is the error Bind returns from a DelimitedReader or a Sheet that has a
// plan already — one it was opened with, or one bound before.
var ErrAlreadyBound = errors.New("hypertabular: a plan is already bound")

// ErrBusy is the error a DelimitedReader's Read, Bind and Close return when they are called from
// inside one of its own Reads: from the io.Reader it reads, which runs in the middle of one. The
// outer Read is not disturbed, and goes on to deliver its batch; the reader is still there to use
// once it has returned.
var ErrBusy = errors.New("hypertabular: the reader is in the middle of a read; its source can't call back into it")
