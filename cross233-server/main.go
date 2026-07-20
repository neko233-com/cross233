package main

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/neko233-com/cross233/internal/protocol"
)

type config struct {
	bind, password, certFile, keyFile      string
	controlPort, webPort, portMin, portMax int
}

type clientConn struct {
	id   string
	conn net.Conn
	enc  *json.Encoder
	mu   sync.Mutex
}

func (c *clientConn) send(msg protocol.Message) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.enc.Encode(msg)
}

type serviceEntry struct {
	service  protocol.Service
	client   *clientConn
	listener net.Listener
}

type server struct {
	cfg        config
	mu         sync.RWMutex
	services   map[int]*serviceEntry
	pending    map[string]chan net.Conn
	logs       []string
	sessionKey []byte
}

func main() {
	cfg := config{}
	flag.StringVar(&cfg.bind, "bind", "0.0.0.0", "listen address")
	flag.StringVar(&cfg.password, "password", "root", "shared server and web password")
	flag.StringVar(&cfg.certFile, "cert", "cross233-cert.pem", "TLS certificate path")
	flag.StringVar(&cfg.keyFile, "key", "cross233-key.pem", "TLS key path")
	flag.IntVar(&cfg.controlPort, "control-port", 7710, "TLS control port")
	flag.IntVar(&cfg.webPort, "web-port", 7711, "web management port")
	flag.IntVar(&cfg.portMin, "port-min", 7712, "first public TCP port")
	flag.IntVar(&cfg.portMax, "port-max", 7720, "last public TCP port")
	flag.Parse()
	if cfg.password == "" || cfg.portMin > cfg.portMax {
		log.Fatal("password required and port range must be valid")
	}

	cert, err := loadOrCreateCert(cfg.certFile, cfg.keyFile)
	if err != nil {
		log.Fatal(err)
	}
	s := &server{cfg: cfg, services: map[int]*serviceEntry{}, pending: map[string]chan net.Conn{}, sessionKey: randomBytes(32)}
	tlsListener, err := tls.Listen("tcp", net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.controlPort)), &tls.Config{Certificates: []tls.Certificate{cert}, MinVersion: tls.VersionTLS13})
	if err != nil {
		log.Fatal(err)
	}
	go s.acceptControl(tlsListener)
	go func() {
		log.Fatal(http.ListenAndServeTLS(net.JoinHostPort(cfg.bind, fmt.Sprint(cfg.webPort)), cfg.certFile, cfg.keyFile, s.webHandler()))
	}()
	s.addLog("server started: control %d, web %d, public %d-%d", cfg.controlPort, cfg.webPort, cfg.portMin, cfg.portMax)
	log.Printf("cross233 server: TLS control :%d, web :%d, public ports %d-%d", cfg.controlPort, cfg.webPort, cfg.portMin, cfg.portMax)
	select {}
}

func (s *server) acceptControl(listener net.Listener) {
	for {
		conn, err := listener.Accept()
		if err != nil {
			log.Printf("control accept: %v", err)
			continue
		}
		go s.handleControl(conn)
	}
}

func (s *server) handleControl(conn net.Conn) {
	dec := json.NewDecoder(conn)
	var hello protocol.Message
	if err := dec.Decode(&hello); err != nil {
		conn.Close()
		return
	}
	if hello.Password != s.cfg.password {
		json.NewEncoder(conn).Encode(protocol.Message{Type: "error", Error: "invalid password"})
		conn.Close()
		return
	}
	switch hello.Type {
	case "client":
		s.handleClient(conn, dec, hello)
		conn.Close()
	case "tunnel":
		if !s.handleTunnel(conn, hello) {
			conn.Close()
		}
	default:
		json.NewEncoder(conn).Encode(protocol.Message{Type: "error", Error: "invalid connection type"})
		conn.Close()
	}
}

func (s *server) handleClient(conn net.Conn, dec *json.Decoder, hello protocol.Message) {
	if hello.ClientID == "" {
		hello.ClientID = randomID()
	}
	client := &clientConn{id: hello.ClientID, conn: conn, enc: json.NewEncoder(conn)}
	if err := s.register(client, hello.Services); err != nil {
		client.send(protocol.Message{Type: "error", Error: err.Error()})
		return
	}
	defer s.removeClient(client)
	client.send(protocol.Message{Type: "ready"})
	s.addLog("client %s connected", client.id)
	for {
		var msg protocol.Message
		if err := dec.Decode(&msg); err != nil {
			return
		}
	}
}

func (s *server) register(client *clientConn, requested []protocol.Service) error {
	if len(requested) == 0 {
		return errors.New("at least one service required")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, svc := range requested {
		if svc.Name == "" || svc.RemotePort < s.cfg.portMin || svc.RemotePort > s.cfg.portMax {
			return fmt.Errorf("invalid service %q on port %d", svc.Name, svc.RemotePort)
		}
		if _, exists := s.services[svc.RemotePort]; exists {
			return fmt.Errorf("public port %d already in use", svc.RemotePort)
		}
	}
	opened := make([]*serviceEntry, 0, len(requested))
	for _, svc := range requested {
		listener, err := net.Listen("tcp", net.JoinHostPort(s.cfg.bind, fmt.Sprint(svc.RemotePort)))
		if err != nil {
			for _, entry := range opened {
				entry.listener.Close()
				delete(s.services, entry.service.RemotePort)
			}
			return err
		}
		entry := &serviceEntry{service: svc, client: client, listener: listener}
		s.services[svc.RemotePort] = entry
		opened = append(opened, entry)
		go s.acceptPublic(entry)
	}
	return nil
}

func (s *server) removeClient(client *clientConn) {
	s.mu.Lock()
	for port, entry := range s.services {
		if entry.client == client {
			entry.listener.Close()
			delete(s.services, port)
		}
	}
	s.mu.Unlock()
	s.addLog("client %s disconnected", client.id)
}

func (s *server) acceptPublic(entry *serviceEntry) {
	for {
		publicConn, err := entry.listener.Accept()
		if err != nil {
			return
		}
		go s.bridgePublic(entry, publicConn)
	}
}

func (s *server) bridgePublic(entry *serviceEntry, publicConn net.Conn) {
	defer publicConn.Close()
	id := randomID()
	waiter := make(chan net.Conn, 1)
	s.mu.Lock()
	s.pending[id] = waiter
	s.mu.Unlock()
	defer func() { s.mu.Lock(); delete(s.pending, id); s.mu.Unlock() }()
	if err := entry.client.send(protocol.Message{Type: "open", ID: id, Service: &entry.service}); err != nil {
		return
	}
	select {
	case tunnel := <-waiter:
		defer tunnel.Close()
		bridge(publicConn, tunnel)
	case <-time.After(15 * time.Second):
		s.addLog("tunnel timed out for %s", entry.service.Name)
	}
}

func (s *server) handleTunnel(conn net.Conn, hello protocol.Message) bool {
	if hello.ID == "" {
		return false
	}
	s.mu.RLock()
	waiter := s.pending[hello.ID]
	s.mu.RUnlock()
	if waiter == nil {
		return false
	}
	select {
	case waiter <- conn:
		return true
	case <-time.After(time.Second):
		return false
	}
	// Ownership moved to bridgePublic after handoff.
}

func bridge(left, right net.Conn) {
	done := make(chan struct{}, 2)
	copyConn := func(dst, src net.Conn) { _, _ = io.Copy(dst, src); done <- struct{}{} }
	go copyConn(left, right)
	go copyConn(right, left)
	<-done
}

func (s *server) addLog(format string, args ...any) {
	line := time.Now().Format("15:04:05") + " " + fmt.Sprintf(format, args...)
	log.Print(line)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.logs = append([]string{line}, s.logs...)
	if len(s.logs) > 100 {
		s.logs = s.logs[:100]
	}
}

func (s *server) webHandler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/login", s.login)
	mux.HandleFunc("/logout", s.logout)
	mux.HandleFunc("/", s.dashboard)
	return mux
}

func (s *server) login(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodGet {
		fmt.Fprint(w, loginPage)
		return
	}
	if r.Method != http.MethodPost || r.FormValue("password") != s.cfg.password {
		http.Error(w, "wrong password", http.StatusUnauthorized)
		return
	}
	http.SetCookie(w, &http.Cookie{Name: "cross233_session", Value: s.sign("ok"), HttpOnly: true, SameSite: http.SameSiteStrictMode, Path: "/", MaxAge: 86400})
	http.Redirect(w, r, "/", http.StatusSeeOther)
}

func (s *server) logout(w http.ResponseWriter, r *http.Request) {
	http.SetCookie(w, &http.Cookie{Name: "cross233_session", Value: "", Path: "/", MaxAge: -1})
	http.Redirect(w, r, "/login", http.StatusSeeOther)
}

func (s *server) dashboard(w http.ResponseWriter, r *http.Request) {
	cookie, err := r.Cookie("cross233_session")
	if err != nil || cookie.Value != s.sign("ok") {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	s.mu.RLock()
	services := make([]*serviceEntry, 0, len(s.services))
	for _, entry := range s.services {
		services = append(services, entry)
	}
	logs := append([]string(nil), s.logs...)
	s.mu.RUnlock()
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	fmt.Fprint(w, dashboardPage(services, logs))
}

func (s *server) sign(value string) string {
	return fmt.Sprintf("%x", hmacSHA256(s.sessionKey, []byte(value)))
}

func loadOrCreateCert(certFile, keyFile string) (tls.Certificate, error) {
	if _, err := os.Stat(certFile); err == nil {
		return tls.LoadX509KeyPair(certFile, keyFile)
	}
	if err := os.MkdirAll(filepath.Dir(certFile), 0700); err != nil && filepath.Dir(certFile) != "." {
		return tls.Certificate{}, err
	}
	priv, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return tls.Certificate{}, err
	}
	serial, _ := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	tmpl := x509.Certificate{SerialNumber: serial, Subject: pkix.Name{CommonName: "cross233"}, NotBefore: time.Now().Add(-time.Minute), NotAfter: time.Now().AddDate(5, 0, 0), KeyUsage: x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}, DNSNames: []string{"cross233"}}
	der, err := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &priv.PublicKey, priv)
	if err != nil {
		return tls.Certificate{}, err
	}
	if err := os.WriteFile(certFile, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0600); err != nil {
		return tls.Certificate{}, err
	}
	keyBytes, err := x509.MarshalPKCS8PrivateKey(priv)
	if err != nil {
		return tls.Certificate{}, err
	}
	if err := os.WriteFile(keyFile, pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyBytes}), 0600); err != nil {
		return tls.Certificate{}, err
	}
	return tls.X509KeyPair(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyBytes}))
}

func randomBytes(n int) []byte {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	return b
}
func randomID() string { return fmt.Sprintf("%x", randomBytes(16)) }
