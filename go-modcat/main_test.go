package main

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestGetJSONDiagnosesHosting404WithoutEchoingHTML(t *testing.T) {
	const marker = "private-hosting-panel-detail"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.WriteHeader(http.StatusNotFound)
		_, _ = w.Write([]byte("<!doctype html><title>404</title>" + marker))
	}))
	defer server.Close()

	_, err := getJSON(server.Client(), server.URL)
	if err == nil || !strings.Contains(err.Error(), "HTML-страницу 404") {
		t.Fatalf("expected actionable catalog 404 error, got %v", err)
	}
	if strings.Contains(err.Error(), marker) || strings.Contains(err.Error(), "<!doctype") {
		t.Fatalf("response body leaked into error: %v", err)
	}
}

func TestGetJSONDiagnosesHTMLServedAsSuccess(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		_, _ = w.Write([]byte("<html><body>not an API</body></html>"))
	}))
	defer server.Close()

	_, err := getJSON(server.Client(), server.URL)
	if err == nil || !strings.Contains(err.Error(), "API каталога вернул HTML вместо JSON") {
		t.Fatalf("expected actionable invalid API response, got %v", err)
	}
}
