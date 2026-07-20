package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/neko233-com/cross233/internal/protocol"
)

type clientConfig struct {
	Server   string             `json:"server"`
	AuthKey  string             `json:"auth_key"`
	KeyFile  string             `json:"key_file"`
	CAFile   string             `json:"ca_file"`
	Insecure bool               `json:"insecure"`
	ClientID string             `json:"client_id"`
	Services []protocol.Service `json:"services"`
}

func main() {
	var cfg clientConfig
	var configPath, serviceSpec string
	flag.StringVar(&configPath, "config", "", "JSON config path")
	flag.StringVar(&cfg.Server, "server", "", "server control address, e.g. host:7710")
	flag.StringVar(&cfg.AuthKey, "auth-key", "", "shared access key")
	flag.StringVar(&cfg.KeyFile, "key-file", "", "access key file path")
	flag.StringVar(&cfg.CAFile, "ca", "", "server certificate PEM path")
	flag.BoolVar(&cfg.Insecure, "insecure", false, "skip TLS verification; testing only")
	flag.StringVar(&cfg.ClientID, "client-id", "", "stable client identifier")
	flag.StringVar(&serviceSpec, "services", "", "name:public-port:local-host:local-port entries")
	flag.Parse()
	if configPath != "" {
		data, err := os.ReadFile(configPath)
		if err != nil {
			log.Fatal(err)
		}
		if err := json.Unmarshal(data, &cfg); err != nil {
			log.Fatal(err)
		}
	}
	if serviceSpec != "" {
		services, err := parseServices(serviceSpec)
		if err != nil {
			log.Fatal(err)
		}
		cfg.Services = services
	}
	if cfg.KeyFile != "" {
		data, err := os.ReadFile(cfg.KeyFile)
		if err != nil {
			log.Fatal(err)
		}
		cfg.AuthKey = strings.TrimSpace(string(data))
	}
	if cfg.Server == "" || cfg.AuthKey == "" || len(cfg.Services) == 0 {
		flag.Usage()
		log.Fatal("server, auth key and at least one service required")
	}
	if !cfg.Insecure && cfg.CAFile == "" {
		log.Fatal("TLS verification requires -ca; use -insecure only for local testing")
	}
	if cfg.ClientID == "" {
		host, _ := os.Hostname()
		cfg.ClientID = host
	}
	tlsConfig, err := tlsConfig(cfg)
	if err != nil {
		log.Fatal(err)
	}
	for {
		if err := runClient(cfg, tlsConfig); err != nil {
			log.Printf("disconnected: %v; retrying in 3s", err)
		}
		time.Sleep(3 * time.Second)
	}
}

func tlsConfig(cfg clientConfig) (*tls.Config, error) {
	conf := &tls.Config{MinVersion: tls.VersionTLS13, ServerName: "cross233", InsecureSkipVerify: cfg.Insecure} // #nosec G402: explicit test-only flag.
	if cfg.CAFile != "" {
		pemData, err := os.ReadFile(cfg.CAFile)
		if err != nil {
			return nil, err
		}
		pool := x509.NewCertPool()
		if !pool.AppendCertsFromPEM(pemData) {
			return nil, errors.New("invalid CA certificate")
		}
		conf.RootCAs = pool
	}
	return conf, nil
}

func runClient(cfg clientConfig, tlsConfig *tls.Config) error {
	conn, err := tls.Dial("tcp", cfg.Server, tlsConfig)
	if err != nil {
		return err
	}
	defer conn.Close()
	enc := json.NewEncoder(conn)
	hello := protocol.Message{Type: "client", ClientID: cfg.ClientID, Services: cfg.Services}
	if err := enc.Encode(hello); err != nil {
		return err
	}
	dec := json.NewDecoder(conn)
	if err := authenticate(dec, enc, cfg.AuthKey, hello); err != nil {
		return err
	}
	var ready protocol.Message
	if err := dec.Decode(&ready); err != nil {
		return err
	}
	if ready.Type == "error" {
		return errors.New(ready.Error)
	}
	if ready.Type != "ready" {
		return fmt.Errorf("unexpected server reply %q", ready.Type)
	}
	log.Printf("connected to %s with %d service(s)", cfg.Server, len(cfg.Services))
	for {
		var msg protocol.Message
		if err := dec.Decode(&msg); err != nil {
			return err
		}
		if msg.Type == "open" && msg.Service != nil {
			go openTunnel(cfg, tlsConfig, msg)
		}
	}
}

func openTunnel(cfg clientConfig, tlsConfig *tls.Config, msg protocol.Message) {
	tunnel, err := tls.Dial("tcp", cfg.Server, tlsConfig)
	if err != nil {
		log.Printf("tunnel %s: %v", msg.ID, err)
		return
	}
	defer tunnel.Close()
	enc := json.NewEncoder(tunnel)
	dec := json.NewDecoder(tunnel)
	hello := protocol.Message{Type: "tunnel", ID: msg.ID}
	if err := enc.Encode(hello); err != nil {
		return
	}
	if err := authenticate(dec, enc, cfg.AuthKey, hello); err != nil {
		return
	}
	local, err := net.DialTimeout("tcp", msg.Service.LocalAddr, 10*time.Second)
	if err != nil {
		log.Printf("%s local dial %s: %v", msg.Service.Name, msg.Service.LocalAddr, err)
		return
	}
	defer local.Close()
	bridge(tunnel, local)
}

func authenticate(dec *json.Decoder, enc *json.Encoder, key string, hello protocol.Message) error {
	var challenge protocol.Message
	if err := dec.Decode(&challenge); err != nil {
		return err
	}
	if challenge.Type == "error" {
		return errors.New(challenge.Error)
	}
	if challenge.Type != "challenge" || challenge.Nonce == "" {
		return errors.New("invalid server challenge")
	}
	proof := makeAuthProof(key, hello, challenge.Nonce)
	return enc.Encode(protocol.Message{Type: "auth", Proof: base64.RawURLEncoding.EncodeToString(proof)})
}

func makeAuthProof(key string, hello protocol.Message, nonce string) []byte {
	data := strings.Join([]string{"cross233/v1", hello.Type, hello.ClientID, hello.ID, nonce}, "\x00")
	h := hmac.New(sha256.New, []byte(key))
	_, _ = h.Write([]byte(data))
	return h.Sum(nil)
}

func bridge(left, right net.Conn) {
	var wg sync.WaitGroup
	wg.Add(2)
	copyConn := func(dst, src net.Conn) { defer wg.Done(); _, _ = io.Copy(dst, src) }
	go copyConn(left, right)
	go copyConn(right, left)
	wg.Wait()
}

func parseServices(input string) ([]protocol.Service, error) {
	var services []protocol.Service
	for _, item := range strings.Split(input, ",") {
		parts := strings.Split(item, ":")
		if len(parts) != 4 || parts[0] == "" || parts[2] == "" {
			return nil, fmt.Errorf("invalid service %q", item)
		}
		port, err := strconv.Atoi(parts[1])
		if err != nil {
			return nil, err
		}
		localPort, err := strconv.Atoi(parts[3])
		if err != nil {
			return nil, err
		}
		services = append(services, protocol.Service{Name: parts[0], RemotePort: port, LocalAddr: net.JoinHostPort(parts[2], strconv.Itoa(localPort))})
	}
	return services, nil
}
