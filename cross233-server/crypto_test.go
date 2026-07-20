package main

import (
	"bytes"
	"encoding/hex"
	"testing"
)

func TestHMACSHA256Consistency(t *testing.T) {
	key := []byte("test-key-12345678901234567890")
	data := []byte("some data to sign")
	out1 := hmacSHA256(key, data)
	out2 := hmacSHA256(key, data)
	if !bytes.Equal(out1, out2) {
		t.Errorf("hmacSHA256 is not deterministic: %x != %x", out1, out2)
	}
	if len(out1) != 32 {
		t.Errorf("output length = %d, want 32", len(out1))
	}
}

func TestHMACSHA256DifferentKeys(t *testing.T) {
	data := []byte("same data")
	out1 := hmacSHA256([]byte("key-one"), data)
	out2 := hmacSHA256([]byte("key-two"), data)
	if bytes.Equal(out1, out2) {
		t.Error("different keys produced same HMAC")
	}
}

func TestHMACSHA256DifferentData(t *testing.T) {
	key := []byte("same-key-12345678901234567890")
	out1 := hmacSHA256(key, []byte("data-a"))
	out2 := hmacSHA256(key, []byte("data-b"))
	if bytes.Equal(out1, out2) {
		t.Error("different data produced same HMAC")
	}
}

func TestHMACSHA256KnownVector(t *testing.T) {
	key, _ := hex.DecodeString("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b")
	data := []byte("Hi There")
	got := hmacSHA256(key, data)
	if len(got) != 32 {
		t.Fatalf("output length = %d, want 32", len(got))
	}
	got2 := hmacSHA256(key, data)
	if !bytes.Equal(got, got2) {
		t.Error("HMAC not deterministic across calls")
	}
}

func TestRandomBytesZeroLength(t *testing.T) {
	b := randomBytes(0)
	if len(b) != 0 {
		t.Errorf("randomBytes(0) returned %d bytes", len(b))
	}
}
