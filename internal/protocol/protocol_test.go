package protocol

import (
	"encoding/json"
	"testing"
)

func TestMessageJSON(t *testing.T) {
	msg := Message{
		Type:     "client",
		ClientID: "abc",
		Services: []Service{
			{Name: "web", RemotePort: 7712, LocalAddr: "127.0.0.1:8080"},
		},
	}
	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatal(err)
	}
	var decoded Message
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatal(err)
	}
	if decoded.Type != "client" {
		t.Errorf("type: %q", decoded.Type)
	}
	if decoded.ClientID != "abc" {
		t.Errorf("client_id: %q", decoded.ClientID)
	}
	if len(decoded.Services) != 1 {
		t.Errorf("services: %d", len(decoded.Services))
	}
}

func TestMessageOmitEmpty(t *testing.T) {
	msg := Message{Type: "ready"}
	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatal(err)
	}
	s := string(data)
	if s == "" {
		t.Fatal("empty marshal")
	}
	// Optional fields should be omitted
	if contains(s, "client_id") {
		t.Error("client_id should be omitted")
	}
	if contains(s, "nonce") {
		t.Error("nonce should be omitted")
	}
	if contains(s, "proof") {
		t.Error("proof should be omitted")
	}
	if contains(s, "services") {
		t.Error("services should be omitted")
	}
	if contains(s, "service") {
		t.Error("service should be omitted")
	}
	if contains(s, "error") {
		t.Error("error should be omitted")
	}
}

func contains(s, sub string) bool {
	return len(s) >= len(sub) && (s == sub || len(s) > 0 && containsImpl(s, sub))
}

func containsImpl(s, sub string) bool {
	for i := 0; i <= len(s)-len(sub); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}

func TestMessageChallenge(t *testing.T) {
	msg := Message{Type: "challenge", Nonce: "abc123"}
	data, _ := json.Marshal(msg)
	var decoded Message
	json.Unmarshal(data, &decoded)
	if decoded.Nonce != "abc123" {
		t.Errorf("nonce: %q", decoded.Nonce)
	}
}

func TestMessageAuth(t *testing.T) {
	msg := Message{Type: "auth", Proof: "base64proof"}
	data, _ := json.Marshal(msg)
	var decoded Message
	json.Unmarshal(data, &decoded)
	if decoded.Proof != "base64proof" {
		t.Errorf("proof: %q", decoded.Proof)
	}
}

func TestMessageOpen(t *testing.T) {
	svc := &Service{Name: "web", RemotePort: 7712, LocalAddr: "127.0.0.1:8080"}
	msg := Message{Type: "open", ID: "tunnel-123", Service: svc}
	data, _ := json.Marshal(msg)
	var decoded Message
	json.Unmarshal(data, &decoded)
	if decoded.ID != "tunnel-123" {
		t.Errorf("id: %q", decoded.ID)
	}
	if decoded.Service == nil || decoded.Service.Name != "web" {
		t.Errorf("service: %+v", decoded.Service)
	}
}

func TestMessageError(t *testing.T) {
	msg := Message{Type: "error", Error: "something went wrong"}
	data, _ := json.Marshal(msg)
	var decoded Message
	json.Unmarshal(data, &decoded)
	if decoded.Error != "something went wrong" {
		t.Errorf("error: %q", decoded.Error)
	}
}

func TestServiceJSON(t *testing.T) {
	svc := Service{Name: "ssh", RemotePort: 7713, LocalAddr: "192.168.1.1:22"}
	data, err := json.Marshal(svc)
	if err != nil {
		t.Fatal(err)
	}
	var decoded Service
	json.Unmarshal(data, &decoded)
	if decoded.Name != "ssh" || decoded.RemotePort != 7713 || decoded.LocalAddr != "192.168.1.1:22" {
		t.Errorf("decoded: %+v", decoded)
	}
}

func TestServiceOmitEmptyLocalAddr(t *testing.T) {
	svc := Service{Name: "web", RemotePort: 7712}
	data, _ := json.Marshal(svc)
	s := string(data)
	if contains(s, "local_addr") {
		t.Error("local_addr should be omitted when empty")
	}
}

func TestMessageNilService(t *testing.T) {
	msg := Message{Type: "challenge", Nonce: "abc"}
	data, _ := json.Marshal(msg)
	var decoded Message
	json.Unmarshal(data, &decoded)
	if decoded.Service != nil {
		t.Error("service should be nil when omitted")
	}
}

func TestMessageNilServices(t *testing.T) {
	msg := Message{Type: "ready"}
	data, _ := json.Marshal(msg)
	var decoded Message
	json.Unmarshal(data, &decoded)
	if decoded.Services != nil {
		t.Error("services should be nil when omitted")
	}
}

func TestMessageRoundTrip(t *testing.T) {
	messages := []Message{
		{Type: "client", ClientID: "host1", Services: []Service{{Name: "web", RemotePort: 7712}}},
		{Type: "tunnel", ID: "abc123"},
		{Type: "challenge", Nonce: "xyz789"},
		{Type: "auth", Proof: "proof123"},
		{Type: "open", ID: "tunnel-456", Service: &Service{Name: "ssh", RemotePort: 7713}},
		{Type: "ready"},
		{Type: "error", Error: "bad request"},
	}

	for _, msg := range messages {
		data, err := json.Marshal(msg)
		if err != nil {
			t.Fatalf("marshal %q: %v", msg.Type, err)
		}
		var decoded Message
		if err := json.Unmarshal(data, &decoded); err != nil {
			t.Fatalf("unmarshal %q: %v", msg.Type, err)
		}
		if decoded.Type != msg.Type {
			t.Errorf("type mismatch: %q vs %q", decoded.Type, msg.Type)
		}
	}
}

func TestMessagePartial(t *testing.T) {
	// JSON with extra fields should not break unmarshaling
	data := []byte(`{"type":"client","client_id":"abc","unknown_field":"value"}`)
	var msg Message
	if err := json.Unmarshal(data, &msg); err != nil {
		t.Fatal(err)
	}
	if msg.Type != "client" || msg.ClientID != "abc" {
		t.Errorf("unexpected: %+v", msg)
	}
}
