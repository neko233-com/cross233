package protocol

// Message is framed as one JSON object per line over a TLS connection.
type Message struct {
	Type     string    `json:"type"`
	Password string    `json:"password,omitempty"`
	ClientID string    `json:"client_id,omitempty"`
	ID       string    `json:"id,omitempty"`
	Services []Service `json:"services,omitempty"`
	Service  *Service  `json:"service,omitempty"`
	Error    string    `json:"error,omitempty"`
}

type Service struct {
	Name       string `json:"name"`
	RemotePort int    `json:"remote_port"`
	LocalAddr  string `json:"local_addr,omitempty"`
}
