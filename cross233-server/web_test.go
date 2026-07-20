package main

import (
	"bytes"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/neko233-com/cross233/internal/protocol"
)

func TestDashboardPageRenders(t *testing.T) {
	services := []*serviceEntry{
		{
			service:  protocol.Service{Name: "web", RemotePort: 7712, LocalAddr: "127.0.0.1:8080"},
			client:   &clientConn{id: "host-a"},
			listener: nil,
		},
	}
	logs := []string{"12:00:00 client host-a connected", "12:00:01 service web registered"}
	html := dashboardPage(services, logs)
	if !strings.Contains(html, "web") {
		t.Error("dashboard missing service name")
	}
	if !strings.Contains(html, "7712") {
		t.Error("dashboard missing port")
	}
	if !strings.Contains(html, "127.0.0.1:8080") {
		t.Error("dashboard missing local addr")
	}
	if !strings.Contains(html, "host-a") {
		t.Error("dashboard missing client id")
	}
	if !strings.Contains(html, "12:00:00") {
		t.Error("dashboard missing log entry")
	}
}

func TestDashboardPageEmpty(t *testing.T) {
	html := dashboardPage(nil, nil)
	if !strings.Contains(html, "No connected services") {
		t.Error("empty dashboard should show 'No connected services'")
	}
}

func TestDashboardPageXSS(t *testing.T) {
	services := []*serviceEntry{
		{
			service:  protocol.Service{Name: "<script>alert(1)</script>", RemotePort: 7712, LocalAddr: "127.0.0.1:8080"},
			client:   &clientConn{id: "evil"},
			listener: nil,
		},
	}
	html := dashboardPage(services, nil)
	if strings.Contains(html, "<script>") {
		t.Error("dashboard does not escape HTML — XSS vulnerability")
	}
}

func TestEsc(t *testing.T) {
	tests := []struct{ in, want string }{
		{"hello", "hello"},
		{"<b>", "&lt;b&gt;"},
		{`"a"`, "&#34;a&#34;"},
	}
	for _, tt := range tests {
		got := esc(tt.in)
		if got != tt.want {
			t.Errorf("esc(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestLoginRedirectsToLogin(t *testing.T) {
	s := &server{sessionKey: randomBytes(32)}
	req := httptest.NewRequest(http.MethodGet, "/login", nil)
	w := httptest.NewRecorder()
	s.login(w, req)
	if w.Code != http.StatusOK {
		t.Errorf("GET /login status = %d, want 200", w.Code)
	}
	if !strings.Contains(w.Body.String(), "cross233") {
		t.Error("login page should contain 'cross233'")
	}
}

func TestDashboardRedirectsUnauthenticated(t *testing.T) {
	s := &server{sessionKey: randomBytes(32)}
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	w := httptest.NewRecorder()
	s.dashboard(w, req)
	if w.Code != http.StatusSeeOther {
		t.Errorf("unauthenticated dashboard status = %d, want %d", w.Code, http.StatusSeeOther)
	}
}

func TestHealthHandler(t *testing.T) {
	s := &server{}
	req := httptest.NewRequest(http.MethodGet, "/healthz", nil)
	w := httptest.NewRecorder()
	s.health(w, req)
	if w.Code != http.StatusOK {
		t.Errorf("healthz status = %d, want 200", w.Code)
	}
	if !strings.Contains(w.Body.String(), "ok") {
		t.Error("healthz should return ok")
	}
}

func TestHealthMethodNotAllowed(t *testing.T) {
	s := &server{}
	req := httptest.NewRequest(http.MethodPost, "/healthz", nil)
	w := httptest.NewRecorder()
	s.health(w, req)
	if w.Code != http.StatusMethodNotAllowed {
		t.Errorf("POST /healthz status = %d, want 405", w.Code)
	}
}

func TestLoginPOSTWrongKey(t *testing.T) {
	s := &server{cfg: config{authKey: "correct-key-here-1234567890"}, sessionKey: randomBytes(32)}
	body := bytes.NewBufferString("auth_key=wrong-key")
	req := httptest.NewRequest(http.MethodPost, "/login", body)
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	w := httptest.NewRecorder()
	s.login(w, req)
	if w.Code != http.StatusUnauthorized {
		t.Errorf("POST /login with wrong key status = %d, want 401", w.Code)
	}
}

func TestLoginPOSTCorrectKey(t *testing.T) {
	s := &server{cfg: config{authKey: "correct-key-here-1234567890"}, sessionKey: randomBytes(32)}
	body := bytes.NewBufferString("auth_key=correct-key-here-1234567890")
	req := httptest.NewRequest(http.MethodPost, "/login", body)
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	w := httptest.NewRecorder()
	s.login(w, req)
	if w.Code != http.StatusSeeOther {
		t.Errorf("POST /login with correct key status = %d, want %d", w.Code, http.StatusSeeOther)
	}
	if w.Header().Get("Set-Cookie") == "" {
		t.Error("login should set session cookie")
	}
}

func TestLogout(t *testing.T) {
	s := &server{}
	req := httptest.NewRequest(http.MethodGet, "/logout", nil)
	w := httptest.NewRecorder()
	s.logout(w, req)
	if w.Code != http.StatusSeeOther {
		t.Errorf("logout status = %d, want %d", w.Code, http.StatusSeeOther)
	}
	cookie := w.Header().Get("Set-Cookie")
	if !strings.Contains(cookie, "cross233_session") {
		t.Error("logout should clear session cookie")
	}
}

func TestRequireAPIMethodNotAllowed(t *testing.T) {
	s := &server{}
	req := httptest.NewRequest(http.MethodPost, "/api/v1/status", nil)
	w := httptest.NewRecorder()
	if s.requireAPI(w, req) {
		t.Error("requireAPI should reject non-GET methods")
	}
}

func TestRequireAPIUnauthorized(t *testing.T) {
	s := &server{cfg: config{authKey: "real-key-12345678901234567890"}}
	req := httptest.NewRequest(http.MethodGet, "/api/v1/status", nil)
	w := httptest.NewRecorder()
	if s.requireAPI(w, req) {
		t.Error("requireAPI should reject without auth")
	}
}

func TestRequireAPISuccess(t *testing.T) {
	s := &server{cfg: config{authKey: "real-key-12345678901234567890"}}
	req := httptest.NewRequest(http.MethodGet, "/api/v1/status", nil)
	req.Header.Set("Authorization", "Bearer real-key-12345678901234567890")
	w := httptest.NewRecorder()
	if !s.requireAPI(w, req) {
		t.Error("requireAPI should accept valid auth")
	}
}
