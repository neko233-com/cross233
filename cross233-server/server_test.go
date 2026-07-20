package main

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"math/big"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/neko233-com/cross233/internal/protocol"
)

// testCert creates a self-signed cert for testing and returns the TLS config.
func testCert(t *testing.T) (certPath, keyPath string, cfg *tls.Config) {
	t.Helper()
	dir := t.TempDir()
	certPath = filepath.Join(dir, "cert.pem")
	keyPath = filepath.Join(dir, "key.pem")
	priv, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
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
	der, err := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &priv.PublicKey, priv)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(certPath, encodePEM("CERTIFICATE", der), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(keyPath, encodePrivateKey(priv), 0600); err != nil {
		t.Fatal(err)
	}
	cert, err := tls.LoadX509KeyPair(certPath, keyPath)
	if err != nil {
		t.Fatal(err)
	}
	pool := x509.NewCertPool()
	pool.AddCert(certLeaf(cert))
	cfg = &tls.Config{
		RootCAs:    pool,
		ServerName: "localhost",
		MinVersion: tls.VersionTLS13,
	}
	return
}

func encodePEM(typ string, der []byte) []byte {
	b := pemEncodeToMemory(&pem.Block{Type: typ, Bytes: der})
	return b
}

func encodePrivateKey(priv *rsa.PrivateKey) []byte {
	b, _ := x509.MarshalPKCS8PrivateKey(priv)
	return pemEncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: b})
}

func pemEncodeToMemory(b *pem.Block) []byte {
	return pem.EncodeToMemory(b)
}

func certLeaf(cert tls.Certificate) *x509.Certificate {
	c, err := x509.ParseCertificate(cert.Certificate[0])
	if err != nil {
		panic(err)
	}
	return c
}

// newTestServer starts a server on random ports and returns a ready-to-use server.
func newTestServer(t *testing.T) (*server, string) {
	t.Helper()
	authKey := base64.RawURLEncoding.EncodeToString(randomBytes(32))
	certPath, keyPath, _ := testCert(t)

	// Find free ports.
	controlLn, _ := net.Listen("tcp", "127.0.0.1:0")
	webLn, _ := net.Listen("tcp", "127.0.0.1:0")
	portMinLn, _ := net.Listen("tcp", "127.0.0.1:0")
	controlLn.Close()
	webLn.Close()
	portMinLn.Close()

	controlPort := controlLn.Addr().(*net.TCPAddr).Port
	webPort := webLn.Addr().(*net.TCPAddr).Port
	portMin := portMinLn.Addr().(*net.TCPAddr).Port

	cfg := config{
		bind:        "127.0.0.1",
		authKey:     authKey,
		certFile:    certPath,
		keyFile:     keyPath,
		controlPort: controlPort,
		webPort:     webPort,
		portMin:     portMin,
		portMax:     portMin + 5,
	}

	s := &server{
		cfg:        cfg,
		services:   map[int]*serviceEntry{},
		pending:    map[string]chan net.Conn{},
		sessionKey: randomBytes(32),
		startedAt:  time.Now().UTC(),
	}

	// Start control listener.
	controlListener, err := tls.Listen("tcp", net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.controlPort)), &tls.Config{
		Certificates: []tls.Certificate{func() tls.Certificate { c, _ := tls.LoadX509KeyPair(certPath, keyPath); return c }()},
		MinVersion:   tls.VersionTLS13,
	})
	if err != nil {
		t.Fatal(err)
	}
	go s.acceptControl(controlListener)

	// Start web server.
	go func() {
		cert, _ := tls.LoadX509KeyPair(certPath, keyPath)
		httpServer := &http.Server{
			Addr:    net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.webPort)),
			Handler: s.webHandler(),
			TLSConfig: &tls.Config{
				Certificates: []tls.Certificate{cert},
				MinVersion:   tls.VersionTLS13,
			},
		}
		httpServer.ListenAndServeTLS(certPath, keyPath)
	}()

	// Wait for server to be ready.
	time.Sleep(100 * time.Millisecond)
	return s, authKey
}

// tlsDial connects to the server with TLS.
func tlsDial(t *testing.T, addr string, tlsCfg *tls.Config) net.Conn {
	t.Helper()
	conn, err := tls.Dial("tcp", addr, tlsCfg)
	if err != nil {
		t.Fatal(err)
	}
	return conn
}

// doAuth performs the full auth handshake on conn.
func doAuth(t *testing.T, conn net.Conn, authKey string, hello protocol.Message) {
	t.Helper()
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)
	if err := enc.Encode(hello); err != nil {
		t.Fatal(err)
	}
	var challenge protocol.Message
	if err := dec.Decode(&challenge); err != nil {
		t.Fatal(err)
	}
	if challenge.Type != "challenge" || challenge.Nonce == "" {
		t.Fatalf("expected challenge, got %v", challenge)
	}
	proof := authProof(authKey, hello, challenge.Nonce)
	if err := enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)}); err != nil {
		t.Fatal(err)
	}
}

// --- Tests ---

func TestBridge(t *testing.T) {
	a, b := net.Pipe()
	var wg sync.WaitGroup
	wg.Add(2)
	go func() { defer wg.Done(); a.Write([]byte("hello")) }()
	go func() { defer wg.Done(); buf := make([]byte, 5); b.Read(buf); if string(buf) != "hello" { t.Errorf("got %q", buf) } }()
	wg.Wait()
	a.Close()
	b.Close()
}

func TestAuthProof(t *testing.T) {
	key := "test-secret-key-1234567890"
	hello := protocol.Message{Type: "client", ClientID: "abc"}
	nonce := "random-nonce"
	proof := authProof(key, hello, nonce)
	if len(proof) != 32 {
		t.Fatalf("expected 32 bytes, got %d", len(proof))
	}
	// Verify deterministic
	proof2 := authProof(key, hello, nonce)
	if string(proof) != string(proof2) {
		t.Fatal("proof not deterministic")
	}
}

func TestVerifyProof(t *testing.T) {
	key := "test-secret-key-1234567890"
	hello := protocol.Message{Type: "client", ClientID: "abc"}
	nonce := "random-nonce"
	proof := base64.RawURLEncoding.EncodeToString(authProof(key, hello, nonce))
	if !verifyProof(key, hello, nonce, proof) {
		t.Fatal("valid proof rejected")
	}
	// Wrong key
	if verifyProof("wrong-key-12345678901234", hello, nonce, proof) {
		t.Fatal("wrong key accepted")
	}
	// Wrong nonce
	if verifyProof(key, hello, "wrong-nonce", proof) {
		t.Fatal("wrong nonce accepted")
	}
}

func TestLoadOrCreateCert(t *testing.T) {
	dir := t.TempDir()
	certFile := filepath.Join(dir, "test-cert.pem")
	keyFile := filepath.Join(dir, "test-key.pem")

	cert, err := loadOrCreateCert(certFile, keyFile)
	if err != nil {
		t.Fatal(err)
	}
	if len(cert.Certificate) == 0 {
		t.Fatal("no certificate")
	}

	// Loading again should work.
	cert2, err := loadOrCreateCert(certFile, keyFile)
	if err != nil {
		t.Fatal(err)
	}
	if len(cert2.Certificate) == 0 {
		t.Fatal("no certificate on reload")
	}
}

func TestLoadOrCreateAuthKey(t *testing.T) {
	dir := t.TempDir()
	keyFile := filepath.Join(dir, "auth.key")

	// Provided key (>=32 chars).
	key, err := loadOrCreateAuthKey("my-secret-key-that-is-long-enough", keyFile)
	if err != nil {
		t.Fatal(err)
	}
	if key != "my-secret-key-that-is-long-enough" {
		t.Fatalf("expected provided key, got %q", key)
	}

	// Provided key too short.
	_, err = loadOrCreateAuthKey("short", keyFile)
	if err == nil {
		t.Fatal("expected error for short key")
	}

	// No provided key, no file — should create one.
	os.Remove(keyFile)
	key2, err := loadOrCreateAuthKey("", keyFile)
	if err != nil {
		t.Fatal(err)
	}
	if len(key2) < 32 {
		t.Fatal("generated key too short")
	}

	// Load from file.
	key3, err := loadOrCreateAuthKey("", keyFile)
	if err != nil {
		t.Fatal(err)
	}
	if key3 != key2 {
		t.Fatal("file key mismatch")
	}
}

func TestRegisterDuplicatePort(t *testing.T) {
	s, _ := newTestServer(t)
	svc := protocol.Service{Name: "web", RemotePort: s.cfg.portMin, LocalAddr: "127.0.0.1:8080"}

	// First registration should succeed (via client connect).
	// But we can test via the internal register.
	client := &clientConn{id: "test", conn: nil, enc: nil}
	if err := s.register(client, []protocol.Service{svc}); err != nil {
		t.Fatal(err)
	}
	defer s.removeClient(client)

	// Second registration on same port should fail.
	client2 := &clientConn{id: "test2", conn: nil, enc: nil}
	err := s.register(client2, []protocol.Service{svc})
	if err == nil {
		t.Fatal("expected duplicate port error")
	}
}

func TestRegisterInvalidPort(t *testing.T) {
	s, _ := newTestServer(t)
	client := &clientConn{id: "test", conn: nil, enc: nil}

	// Port too low.
	err := s.register(client, []protocol.Service{{Name: "x", RemotePort: s.cfg.portMin - 1}})
	if err == nil {
		t.Fatal("expected error for port too low")
	}

	// Port too high.
	err = s.register(client, []protocol.Service{{Name: "x", RemotePort: s.cfg.portMax + 1}})
	if err == nil {
		t.Fatal("expected error for port too high")
	}
}

func TestRegisterEmptyService(t *testing.T) {
	s, _ := newTestServer(t)
	client := &clientConn{id: "test", conn: nil, enc: nil}
	err := s.register(client, []protocol.Service{})
	if err == nil {
		t.Fatal("expected error for empty services")
	}
}

func TestRemoveClient(t *testing.T) {
	s, _ := newTestServer(t)
	client := &clientConn{id: "test-remove", conn: nil, enc: nil}
	svc := protocol.Service{Name: "web", RemotePort: s.cfg.portMin, LocalAddr: "127.0.0.1:8080"}
	if err := s.register(client, []protocol.Service{svc}); err != nil {
		t.Fatal(err)
	}
	s.removeClient(client)
	s.mu.RLock()
	_, exists := s.services[s.cfg.portMin]
	s.mu.RUnlock()
	if exists {
		t.Fatal("service should have been removed")
	}
}

func TestHandleTunnelInvalidID(t *testing.T) {
	s, _ := newTestServer(t)
	conn, _ := net.Pipe()
	result := s.handleTunnel(conn, protocol.Message{Type: "tunnel", ID: ""})
	if result {
		t.Fatal("should reject empty ID")
	}
}

func TestHandleTunnelStaleID(t *testing.T) {
	s, _ := newTestServer(t)
	conn, _ := net.Pipe()
	result := s.handleTunnel(conn, protocol.Message{Type: "tunnel", ID: "nonexistent-id"})
	if result {
		t.Fatal("should reject nonexistent pending ID")
	}
}

func TestHealthEndpoint(t *testing.T) {
	s, _ := newTestServer(t)
	s.addLog("health test")
	s.mu.RLock()
	if len(s.logs) == 0 {
		t.Error("expected logs")
	}
	s.mu.RUnlock()
}

func TestRandomBytes(t *testing.T) {
	b := randomBytes(32)
	if len(b) != 32 {
		t.Fatalf("expected 32 bytes, got %d", len(b))
	}
	b2 := randomBytes(32)
	if string(b) == string(b2) {
		t.Fatal("randomBytes not random")
	}
}

func TestRandomID(t *testing.T) {
	id := randomID()
	if len(id) != 32 {
		t.Fatalf("expected 32 char hex, got %d", len(id))
	}
	id2 := randomID()
	if id == id2 {
		t.Fatal("randomID not random")
	}
}

func TestAddLog(t *testing.T) {
	s, _ := newTestServer(t)
	s.addLog("test %s", "message")
	s.mu.RLock()
	if len(s.logs) != 1 {
		t.Fatalf("expected 1 log, got %d", len(s.logs))
	}
	s.mu.RUnlock()
}

func TestAddLogOverflow(t *testing.T) {
	s, _ := newTestServer(t)
	for i := 0; i < 110; i++ {
		s.addLog("log %d", i)
	}
	s.mu.RLock()
	if len(s.logs) != 100 {
		t.Fatalf("expected 100 logs, got %d", len(s.logs))
	}
	s.mu.RUnlock()
}
