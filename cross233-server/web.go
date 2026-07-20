package main

import (
	"fmt"
	"html/template"
	"strings"
)

const loginPage = `<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><title>cross233</title><style>body{font:16px system-ui;background:#f4f6f8;color:#18212b;display:grid;place-items:center;height:90vh;margin:0}form{background:white;border:1px solid #d6dce2;padding:28px;width:min(340px,calc(100vw - 48px))}input,button{box-sizing:border-box;width:100%;padding:10px;font:inherit;margin-top:12px}button{background:#0d6b5d;color:#fff;border:0;cursor:pointer}</style></head><body><form method="post"><h1>cross233</h1><p>Access key</p><input type="password" name="auth_key" autofocus required><button>Enter</button></form></body></html>`

func dashboardPage(services []*serviceEntry, logs []string) string {
	var rows, events strings.Builder
	for _, svc := range services {
		fmt.Fprintf(&rows, "<tr><td>%s</td><td>%d</td><td>%s</td><td>%s</td></tr>", esc(svc.service.Name), svc.service.RemotePort, esc(svc.service.LocalAddr), esc(svc.client.id))
	}
	if rows.Len() == 0 {
		rows.WriteString("<tr><td colspan=\"4\">No connected services</td></tr>")
	}
	for _, event := range logs {
		fmt.Fprintf(&events, "<li>%s</li>", esc(event))
	}
	return `<!doctype html><html><head><meta http-equiv="refresh" content="10"><meta name="viewport" content="width=device-width,initial-scale=1"><title>cross233</title><style>body{font:15px system-ui;margin:0;background:#f4f6f8;color:#18212b}main{max-width:1024px;margin:36px auto;padding:0 20px}header{display:flex;align-items:center;justify-content:space-between}h1{margin:0;color:#0d6b5d}a{color:#0d6b5d}section{background:#fff;border:1px solid #d6dce2;margin-top:20px;padding:18px}table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:10px;border-bottom:1px solid #e5e9ed;overflow-wrap:anywhere}ol{padding-left:22px;margin:0}li{padding:4px 0;font-family:ui-monospace,monospace;font-size:13px}</style></head><body><main><header><div><h1>cross233</h1><small>Tunnel status. Refreshes every 10 seconds.</small></div><a href="/logout">Log out</a></header><section><h2>Public services</h2><table><thead><tr><th>Name</th><th>Port</th><th>Target</th><th>Client</th></tr></thead><tbody>` + rows.String() + `</tbody></table></section><section><h2>Events</h2><ol>` + events.String() + `</ol></section></main></body></html>`
}

func esc(value string) string { return template.HTMLEscapeString(value) }
