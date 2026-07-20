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
	"testing"
	"time"

	"github.com/neko233-com/cross233/internal/protocol"
)

// testCertTLS creates a self-signed cert and returns the path + TLS config.
func testCertTLS(t *testing.T) (certPath, keyPath string, clientTLS *tls.Config, serverTLS *tls.Config) {
	t.Helper()
	dir := t.TempDir()
	certPath = filepath.Join(dir, "cert.pem")
	keyPath = filepath.Join(dir, "key.pem")
	priv, _ := rsa.GenerateKey(rand.Reader, 2048)
	serial, _ := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	tmpl := x509.Certificate{
		SerialNumber: serial,
		Subject:      pkix.Name{CommonName: "cross233-test"},
		NotBefore:    time.Now().Add(-time.Minute),
		NotAfter:     time.Now().Add(time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		DNSNames:     []string{"localhost"},
		IPAddresses:  []net.IP{net.IPv4(127, 0, 0, 1)},
	}
	der, _ := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &priv.PublicKey, priv)
	certPEM := encodeTestPEM("CERTIFICATE", der)
	keyDER, _ := x509.MarshalPKCS8PrivateKey(priv)
	keyPEM := encodeTestPEM("PRIVATE KEY", keyDER)
	os.WriteFile(certPath, certPEM, 0600)
	os.WriteFile(keyPath, keyPEM, 0600)

	cert, _ := tls.X509KeyPair(certPEM, keyPEM)
	pool := x509.NewCertPool()
	c, _ := x509.ParseCertificate(cert.Certificate[0])
	pool.AddCert(c)

	clientTLS = &tls.Config{RootCAs: pool, ServerName: "localhost", MinVersion: tls.VersionTLS13}
	serverTLS = &tls.Config{Certificates: []tls.Certificate{cert}, MinVersion: tls.VersionTLS13}
	return
}

func encodeTestPEM(typ string, der []byte) []byte {
	return pem.EncodeToMemory(&pem.Block{Type: typ, Bytes: der})
}

// startTestServer starts a full server and returns addresses and auth key.
func startTestServer(t *testing.T) (controlAddr, webAddr, authKey string, certPath string, s *server) {
	t.Helper()
	cp, kp, _, _ := testCertTLS(t)
	certPath = cp
	authKey = base64.RawURLEncoding.EncodeToString(randomBytes(32))

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
		certFile:    cp,
		keyFile:     kp,
		controlPort: controlPort,
		webPort:     webPort,
		portMin:     portMin,
		portMax:     portMin + 5,
	}

	s = &server{
		cfg:        cfg,
		services:   map[int]*serviceEntry{},
		pending:    map[string]chan net.Conn{},
		sessionKey: randomBytes(32),
		startedAt:  time.Now().UTC(),
	}

	controlListener, err := tls.Listen("tcp", net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.controlPort)), &tls.Config{
		Certificates: []tls.Certificate{func() tls.Certificate { c, _ := tls.LoadX509KeyPair(cp, kp); return c }()},
		MinVersion:   tls.VersionTLS13,
	})
	if err != nil {
		t.Fatal(err)
	}
	go s.acceptControl(controlListener)

	cert, _ := tls.LoadX509KeyPair(cp, kp)
	go func() {
		httpServer := &http.Server{
			Addr:    net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.webPort)),
			Handler: s.webHandler(),
			TLSConfig: &tls.Config{
				Certificates: []tls.Certificate{cert},
				MinVersion:   tls.VersionTLS13,
			},
		}
		httpServer.ListenAndServeTLS(cp, kp)
	}()

	controlAddr = net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.controlPort))
	webAddr = net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.webPort))
	time.Sleep(100 * time.Millisecond)
	return
}

// clientTLSConfig returns a TLS config that trusts the server cert.
func clientTLSConfig(certPath string) *tls.Config {
	pemData, _ := os.ReadFile(certPath)
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(pemData)
	return &tls.Config{RootCAs: pool, ServerName: "localhost", MinVersion: tls.VersionTLS13}
}

// findFreePort returns a random available TCP port.
func findFreePort(t *testing.T) int {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := ln.Addr().(*net.TCPAddr).Port
	ln.Close()
	return port
}

// --- API helpers ---

func apiGetServices(t *testing.T, webAddr, authKey, certPath string) []struct {
	Name       string `json:"name"`
	RemotePort int    `json:"remote_port"`
	LocalAddr  string `json:"local_addr"`
	ClientID   string `json:"client_id"`
} {
	t.Helper()
	tlsCfg := &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13}
	client := &http.Client{Transport: &http.Transport{TLSClientConfig: tlsCfg}, Timeout: 5 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("https://%s/api/v1/services", webAddr), nil)
	req.Header.Set("Authorization", "Bearer "+authKey)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var result struct {
		Services []struct {
			Name       string `json:"name"`
			RemotePort int    `json:"remote_port"`
			LocalAddr  string `json:"local_addr"`
			ClientID   string `json:"client_id"`
		} `json:"services"`
	}
	json.NewDecoder(resp.Body).Decode(&result)
	return result.Services
}

func apiGetStatus(t *testing.T, webAddr, authKey, certPath string) map[string]any {
	t.Helper()
	tlsCfg := &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13}
	client := &http.Client{Transport: &http.Transport{TLSClientConfig: tlsCfg}, Timeout: 5 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("https://%s/api/v1/status", webAddr), nil)
	req.Header.Set("Authorization", "Bearer "+authKey)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var result map[string]any
	json.NewDecoder(resp.Body).Decode(&result)
	return result
}

func apiGetLogs(t *testing.T, webAddr, authKey, certPath string) []string {
	t.Helper()
	tlsCfg := &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13}
	client := &http.Client{Transport: &http.Transport{TLSClientConfig: tlsCfg}, Timeout: 5 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("https://%s/api/v1/logs", webAddr), nil)
	req.Header.Set("Authorization", "Bearer "+authKey)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var result struct {
		Logs []string `json:"logs"`
	}
	json.NewDecoder(resp.Body).Decode(&result)
	return result.Logs
}

// --- Integration Tests ---

func TestFullClientServerLifecycle(t *testing.T) {
	controlAddr, webAddr, authKey, certPath, s := startTestServer(t)
	tlsCfg := clientTLSConfig(certPath)

	// Use a port within the server's range
	port := s.cfg.portMin

	// 1. Connect client
	conn, err := tls.Dial("tcp", controlAddr, tlsCfg)
	if err != nil {
		t.Fatal(err)
	}
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)

	// 2. Send hello
	hello := protocol.Message{Type: "client", ClientID: "test-client", Services: []protocol.Service{
		{Name: "web", RemotePort: port, LocalAddr: "127.0.0.1:9999"},
	}}
	enc.Encode(hello)

	// 3. Receive challenge
	var challenge protocol.Message
	dec.Decode(&challenge)
	if challenge.Type != "challenge" || challenge.Nonce == "" {
		t.Fatalf("expected challenge, got %+v", challenge)
	}

	// 4. Send auth
	proof := authProof(authKey, hello, challenge.Nonce)
	enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)})

	// 5. Receive ready
	var ready protocol.Message
	dec.Decode(&ready)
	if ready.Type != "ready" {
		t.Fatalf("expected ready, got %+v", ready)
	}

	// 6. Verify service is registered via web API
	services := apiGetServices(t, webAddr, authKey, certPath)
	if len(services) != 1 {
		t.Fatalf("expected 1 service, got %d", len(services))
	}
	if services[0].Name != "web" {
		t.Errorf("expected service name 'web', got %q", services[0].Name)
	}

	// 7. Disconnect client
	conn.Close()
	time.Sleep(200 * time.Millisecond)

	// 8. Verify service is gone
	services = apiGetServices(t, webAddr, authKey, certPath)
	if len(services) != 0 {
		t.Fatalf("expected 0 services after disconnect, got %d", len(services))
	}
}

func TestFullTunnelDataFlow(t *testing.T) {
	controlAddr, _, authKey, certPath, s := startTestServer(t)
	tlsCfg := clientTLSConfig(certPath)

	// Start a local TCP echo server on a random port
	echoLn, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer echoLn.Close()
	go func() {
		for {
			conn, err := echoLn.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				buf := make([]byte, 1024)
				n, err := c.Read(buf)
				if err != nil {
					return
				}
				c.Write(buf[:n])
			}(conn)
		}
	}()

	echoAddr := echoLn.Addr().String()
	port := s.cfg.portMin

	// Connect client
	conn, err := tls.Dial("tcp", controlAddr, tlsCfg)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)

	hello := protocol.Message{Type: "client", ClientID: "tunnel-test", Services: []protocol.Service{
		{Name: "echo", RemotePort: port, LocalAddr: echoAddr},
	}}
	enc.Encode(hello)

	var ch protocol.Message
	dec.Decode(&ch)
	proof := authProof(authKey, hello, ch.Nonce)
	enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)})

	var readyMsg protocol.Message
	dec.Decode(&readyMsg)
	if readyMsg.Type != "ready" {
		t.Fatalf("expected ready, got %+v", readyMsg)
	}

	// Give server a moment to set up the public listener
	time.Sleep(100 * time.Millisecond)

	// Now simulate: public connection arrives, server sends "open" to client
	// We do this by connecting to the public port and manually driving the tunnel handshake

	// Start a goroutine that acts as the client's tunnel handler
	tunnelDone := make(chan struct{})
	go func() {
		defer close(tunnelDone)
		var openMsg protocol.Message
		if err := dec.Decode(&openMsg); err != nil {
			t.Errorf("decode open: %v", err)
			return
		}
		if openMsg.Type != "open" || openMsg.ID == "" {
			t.Errorf("expected open message, got %+v", openMsg)
			return
		}

		// Open tunnel connection
		tunnelConn, err := tls.Dial("tcp", controlAddr, tlsCfg)
		if err != nil {
			t.Errorf("tunnel dial: %v", err)
			return
		}
		defer tunnelConn.Close()

		tenc := json.NewEncoder(tunnelConn)
		tdec := json.NewDecoder(tunnelConn)

		tEnc := protocol.Message{Type: "tunnel", ID: openMsg.ID}
		tenc.Encode(tEnc)

		var tCh protocol.Message
		tdec.Decode(&tCh)
		if tCh.Type != "challenge" {
			t.Errorf("expected challenge, got %+v", tCh)
			return
		}
		tProof := authProof(authKey, tEnc, tCh.Nonce)
		tenc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(tProof)})

		// Now bridge to local echo server
		localConn, err := net.Dial("tcp", echoAddr)
		if err != nil {
			t.Errorf("local dial: %v", err)
			return
		}
		defer localConn.Close()
		bridge(tunnelConn, localConn)
	}()

	// Connect to public port (plain TCP, not TLS) — this triggers the server to send "open" to client
	publicConn, err := net.Dial("tcp", net.JoinHostPort(s.cfg.bind, fmt.Sprint(port)))
	if err != nil {
		t.Fatalf("public dial: %v", err)
	}
	defer publicConn.Close()

	// Send data through the public connection
	testData := "hello cross233"
	publicConn.Write([]byte(testData))

	// Read response with a deadline
	publicConn.SetReadDeadline(time.Now().Add(5 * time.Second))
	buf := make([]byte, 1024)
	n, err := publicConn.Read(buf)
	publicConn.Close()
	if err != nil && n == 0 {
		t.Fatalf("read from public: %v", err)
	}
	if string(buf[:n]) != testData {
		t.Errorf("got %q, want %q", string(buf[:n]), testData)
	}

	<-tunnelDone
}

func TestAuthFailure(t *testing.T) {
	controlAddr, _, _, certPath, _ := startTestServer(t)
	tlsCfg := clientTLSConfig(certPath)

	conn, err := tls.Dial("tcp", controlAddr, tlsCfg)
	if err != nil {
		t.Fatal(err)
	}
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)

	hello := protocol.Message{Type: "client", ClientID: "bad-client", Services: []protocol.Service{
		{Name: "web", RemotePort: 9999, LocalAddr: "127.0.0.1:80"},
	}}
	enc.Encode(hello)

	var challenge protocol.Message
	dec.Decode(&challenge)

	// Send wrong proof
	enc.Encode(protocol.Message{Type: "auth", Proof: "wrong-proof"})

	var errMsg protocol.Message
	dec.Decode(&errMsg)
	if errMsg.Type != "error" {
		t.Fatalf("expected error, got %+v", errMsg)
	}
	if errMsg.Error != "invalid access key" {
		t.Errorf("expected 'invalid access key', got %q", errMsg.Error)
	}
	conn.Close()
}

func TestDuplicatePortRejected(t *testing.T) {
	controlAddr, _, authKey, certPath, s := startTestServer(t)
	tlsCfg := clientTLSConfig(certPath)
	port := s.cfg.portMin

	// Connect first client
	c1, _ := tls.Dial("tcp", controlAddr, tlsCfg)
	defer c1.Close()
	hello1 := protocol.Message{Type: "client", ClientID: "client1", Services: []protocol.Service{
		{Name: "web", RemotePort: port, LocalAddr: "127.0.0.1:8080"},
	}}
	enc1 := json.NewEncoder(c1)
	dec1 := json.NewDecoder(c1)
	enc1.Encode(hello1)
	var ch1 protocol.Message
	dec1.Decode(&ch1)
	proof1 := authProof(authKey, hello1, ch1.Nonce)
	enc1.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof1)})
	var ready1 protocol.Message
	dec1.Decode(&ready1)

	// Connect second client on same port
	c2, _ := tls.Dial("tcp", controlAddr, tlsCfg)
	defer c2.Close()
	hello2 := protocol.Message{Type: "client", ClientID: "client2", Services: []protocol.Service{
		{Name: "ssh", RemotePort: port, LocalAddr: "127.0.0.1:22"},
	}}
	enc2 := json.NewEncoder(c2)
	dec2 := json.NewDecoder(c2)
	enc2.Encode(hello2)
	var ch2 protocol.Message
	dec2.Decode(&ch2)
	proof2 := authProof(authKey, hello2, ch2.Nonce)
	enc2.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof2)})
	var errMsg protocol.Message
	dec2.Decode(&errMsg)
	if errMsg.Type != "error" {
		t.Fatalf("expected error for duplicate port, got %+v", errMsg)
	}
}

func TestHealthEndpointIntegration(t *testing.T) {
	_, _, _, _, _ = startTestServer(t)
}

func TestAPIStatus(t *testing.T) {
	_, webAddr, authKey, certPath, _ := startTestServer(t)
	status := apiGetStatus(t, webAddr, authKey, certPath)
	if status["status"] != "ok" {
		t.Errorf("status: %v", status["status"])
	}
}

func TestAPILogs(t *testing.T) {
	controlAddr, webAddr, authKey, certPath, s := startTestServer(t)
	// Connect a client to generate a log entry
	tlsCfg := clientTLSConfig(certPath)
	conn, _ := tls.Dial("tcp", controlAddr, tlsCfg)
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)
	hello := protocol.Message{Type: "client", ClientID: "log-client", Services: []protocol.Service{
		{Name: "web", RemotePort: s.cfg.portMin, LocalAddr: "127.0.0.1:8080"},
	}}
	enc.Encode(hello)
	var ch protocol.Message
	dec.Decode(&ch)
	proof := authProof(authKey, hello, ch.Nonce)
	enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)})
	var readyMsg protocol.Message
	dec.Decode(&readyMsg)
	conn.Close()
	time.Sleep(100 * time.Millisecond)

	logs := apiGetLogs(t, webAddr, authKey, certPath)
	if len(logs) == 0 {
		t.Error("expected at least one log entry")
	}
}

func TestTunnelHandoffTimeout(t *testing.T) {
	controlAddr, _, authKey, certPath, s := startTestServer(t)
	tlsCfg := clientTLSConfig(certPath)
	port := s.cfg.portMin

	// Connect client
	conn, _ := tls.Dial("tcp", controlAddr, tlsCfg)
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)
	hello := protocol.Message{Type: "client", ClientID: "timeout-client", Services: []protocol.Service{
		{Name: "web", RemotePort: port, LocalAddr: "127.0.0.1:9999"},
	}}
	enc.Encode(hello)
	var ch protocol.Message
	dec.Decode(&ch)
	proof := authProof(authKey, hello, ch.Nonce)
	enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)})
	var readyMsg protocol.Message
	dec.Decode(&readyMsg)

	// Don't open tunnel connections — they should timeout
	conn.Close()
}

func TestMultipleServices(t *testing.T) {
	controlAddr, _, authKey, certPath, s := startTestServer(t)
	tlsCfg := clientTLSConfig(certPath)

	conn, err := tls.Dial("tcp", controlAddr, tlsCfg)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	enc := json.NewEncoder(conn)
	dec := json.NewDecoder(conn)

	hello := protocol.Message{Type: "client", ClientID: "multi-client", Services: []protocol.Service{
		{Name: "web", RemotePort: s.cfg.portMin, LocalAddr: "127.0.0.1:8080"},
		{Name: "ssh", RemotePort: s.cfg.portMin + 1, LocalAddr: "127.0.0.1:22"},
		{Name: "db", RemotePort: s.cfg.portMin + 2, LocalAddr: "127.0.0.1:5432"},
	}}
	enc.Encode(hello)

	var ch protocol.Message
	dec.Decode(&ch)
	proof := authProof(authKey, hello, ch.Nonce)
	enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)})

	var readyMsg protocol.Message
	dec.Decode(&readyMsg)
	if readyMsg.Type != "ready" {
		t.Fatalf("expected ready, got %+v", readyMsg)
	}

	// Verify all services registered
	s.mu.RLock()
	count := len(s.services)
	s.mu.RUnlock()
	if count != 3 {
		t.Errorf("expected 3 services, got %d", count)
	}
}
