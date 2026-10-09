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

// Find is the ordinal of the first column named name, and whether there is one. The match is
// exact — byte for byte, case and spaces included, nothing trimmed or folded — and the first
// of two columns with the same name is the one found.
func (h Header) Find(name string) (int, bool) {
	for ordinal, candidate := range h {
		if candidate == name {
			return ordinal, true
		}
	}
	return -1, false
}

// FindBytes is Find for a name held as UTF-8 bytes; it allocates nothing.
func (h Header) FindBytes(name []byte) (int, bool) {
	for ordinal, candidate := range h {
		if candidate == string(name) {
			return ordinal, true
		}
	}
	return -1, false
}

// Ordinal is the ordinal of the first column named name, matched as Find matches it — what
// a plan built from names reads best with:
//
//	id, err := header.Ordinal("id")
//
// A name the header does not have is a *NoColumnError that names it, and errors.Is matches
// it to ErrNoColumn.
func (h Header) Ordinal(name string) (int, error) {
	if ordinal, ok := h.Find(name); ok {
		return ordinal, nil
	}
	return -1, &NoColumnError{Name: name}
}

// OrdinalBytes is Ordinal for a name held as UTF-8 bytes.
func (h Header) OrdinalBytes(name []byte) (int, error) {
	if ordinal, ok := h.FindBytes(name); ok {
		return ordinal, nil
	}
	return -1, &NoColumnError{Name: string(name)}
}

// ErrUnbound is the error Read returns from a DelimitedReader or a Sheet opened without a
// plan, until Bind has given it one. It is not the reader's last word: once a plan is
// bound, Read reads.
var ErrUnbound = errors.New("hypertabular: no plan is bound; call Bind before Read")

// ErrAlreadyBound is the error Bind returns from a DelimitedReader or a Sheet that has a
// plan already — one it was opened with, or one bound before.
var ErrAlreadyBound = errors.New("hypertabular: a plan is already bound")
