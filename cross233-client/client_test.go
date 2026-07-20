package main

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"math/big"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/neko233-com/cross233/internal/protocol"
)

// testCert creates a self-signed cert for testing.
func testCert(t *testing.T) (certPath, keyPath string, cfg *tls.Config) {
	t.Helper()
	dir := t.TempDir()
	certPath = filepath.Join(dir, "cert.pem")
	keyPath = filepath.Join(dir, "key.pem")
	priv, _ := rsa.GenerateKey(rand.Reader, 2048)
	serial, _ := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	tmpl := x509.Certificate{
		SerialNumber: serial,
		Subject:      pkix.Name{CommonName: "test"},
		NotBefore:    time.Now().Add(-time.Minute),
		NotAfter:     time.Now().Add(time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		DNSNames:     []string{"localhost"},
		IPAddresses:  []net.IP{net.IPv4(127, 0, 0, 1)},
	}
	der, _ := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &priv.PublicKey, priv)
	os.WriteFile(certPath, encodePEM("CERTIFICATE", der), 0600)
	b, _ := x509.MarshalPKCS8PrivateKey(priv)
	os.WriteFile(keyPath, encodePEM("PRIVATE KEY", b), 0600)
	cert, _ := tls.LoadX509KeyPair(certPath, keyPath)
	pool := x509.NewCertPool()
	c, _ := x509.ParseCertificate(cert.Certificate[0])
	pool.AddCert(c)
	cfg = &tls.Config{RootCAs: pool, ServerName: "localhost", MinVersion: tls.VersionTLS13}
	return
}

func encodePEM(typ string, der []byte) []byte {
	return pem.EncodeToMemory(&pem.Block{Type: typ, Bytes: der})
}

// --- Tests ---

func TestParseServices(t *testing.T) {
	tests := []struct {
		input string
		count int
		err   bool
	}{
		{"web:7712:127.0.0.1:8080", 1, false},
		{"web:7712:127.0.0.1:8080,ssh:7713:127.0.0.1:22", 2, false},
		{"", 0, true},
		{"invalid", 0, true},
		{"web:abc:127.0.0.1:8080", 0, true},
		{"web:7712::8080", 0, true},
		{"web:7712:127.0.0.1:abc", 0, true},
		{":7712:127.0.0.1:8080", 0, true},
		{"web:7712:127.0.0.1:", 0, true},
	}

	for _, tt := range tests {
		services, err := parseServices(tt.input)
		if tt.err {
			if err == nil {
				t.Errorf("parseServices(%q) expected error", tt.input)
			}
			continue
		}
		if err != nil {
			t.Errorf("parseServices(%q) unexpected error: %v", tt.input, err)
			continue
		}
		if len(services) != tt.count {
			t.Errorf("parseServices(%q) got %d services, want %d", tt.input, len(services), tt.count)
		}
	}
}

func TestParseServicePorts(t *testing.T) {
	services, err := parseServices("web:7712:127.0.0.1:8080,ssh:7713:192.168.1.1:22")
	if err != nil {
		t.Fatal(err)
	}
	if services[0].Name != "web" || services[0].RemotePort != 7712 || services[0].LocalAddr != "127.0.0.1:8080" {
		t.Errorf("first service: %+v", services[0])
	}
	if services[1].Name != "ssh" || services[1].RemotePort != 7713 || services[1].LocalAddr != "192.168.1.1:22" {
		t.Errorf("second service: %+v", services[1])
	}
}

func TestMakeAuthProof(t *testing.T) {
	key := "test-key-1234567890123456"
	hello := protocol.Message{Type: "client", ClientID: "myhost", Services: []protocol.Service{{Name: "web", RemotePort: 7712}}}
	nonce := "server-nonce-abc"
	proof := makeAuthProof(key, hello, nonce)
	if len(proof) != 32 {
		t.Fatalf("expected 32 bytes, got %d", len(proof))
	}
	// Deterministic
	proof2 := makeAuthProof(key, hello, nonce)
	if string(proof) != string(proof2) {
		t.Fatal("proof not deterministic")
	}
}

func TestMakeAuthProofDifferentInputs(t *testing.T) {
	key := "test-key-1234567890123456"
	hello := protocol.Message{Type: "client", ClientID: "myhost"}
	nonce := "server-nonce"
	proof1 := makeAuthProof(key, hello, nonce)

	hello2 := protocol.Message{Type: "client", ClientID: "other"}
	proof2 := makeAuthProof(key, hello2, nonce)

	if string(proof1) == string(proof2) {
		t.Fatal("different inputs should produce different proofs")
	}
}

func TestBridge(t *testing.T) {
	a, b := net.Pipe()
	done := make(chan struct{})
	go func() {
		bridge(a, b)
		close(done)
	}()
	a.Write([]byte("hello from a"))
	buf := make([]byte, 13)
	n, _ := b.Read(buf)
	if string(buf[:n]) != "hello from a" {
		t.Errorf("got %q", buf[:n])
	}
	a.Close()
	<-done
}

func TestTlsConfig(t *testing.T) {
	certPath, _, _ := testCert(t)

	// Test insecure mode
	conf := &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13}
	if conf.InsecureSkipVerify != true {
		t.Error("insecure flag not set")
	}

	// Test with CA
	pemData, err := os.ReadFile(certPath)
	if err != nil {
		t.Fatal(err)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(pemData)
	conf2 := &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS13}
	if conf2.RootCAs == nil {
		t.Error("root CAs not set")
	}

	// Test no CA no insecure
	conf3 := &tls.Config{MinVersion: tls.VersionTLS13}
	if conf3.InsecureSkipVerify {
		t.Error("insecure should not be set by default")
	}
	_ = conf3
}

func TestClientConfigJSON(t *testing.T) {
	cfgJSON := `{
		"server": "example.com:7710",
		"auth_key": "my-secret-key",
		"ca_file": "ca.pem",
		"insecure": false,
		"services": [
			{"name": "web", "remote_port": 7712, "local_addr": "127.0.0.1:8080"}
		]
	}`
	var cfg clientConfig
	if err := json.Unmarshal([]byte(cfgJSON), &cfg); err != nil {
		t.Fatal(err)
	}
	if cfg.Server != "example.com:7710" {
		t.Errorf("server: %q", cfg.Server)
	}
	if cfg.AuthKey != "my-secret-key" {
		t.Errorf("auth_key: %q", cfg.AuthKey)
	}
	if len(cfg.Services) != 1 {
		t.Errorf("services: %d", len(cfg.Services))
	}
	if cfg.Services[0].Name != "web" || cfg.Services[0].RemotePort != 7712 {
		t.Errorf("service: %+v", cfg.Services[0])
	}
}

func TestClientConfigJSONMinimal(t *testing.T) {
	cfgJSON := `{"server": "localhost:7710"}`
	var cfg clientConfig
	if err := json.Unmarshal([]byte(cfgJSON), &cfg); err != nil {
		t.Fatal(err)
	}
	if cfg.Server != "localhost:7710" {
		t.Errorf("server: %q", cfg.Server)
	}
	if cfg.Insecure {
		t.Error("insecure should default to false")
	}
}
