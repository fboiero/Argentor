// SPDX-License-Identifier: AGPL-3.0-only
// Argentor Observability Dashboard — WebSocket API client
// Copyright (C) 2025 fboiero

"use strict";

/**
 * AgentorWsClient — connects to the Argentor gateway WebSocket endpoint
 * and emits typed events to registered listeners.
 *
 * Events emitted:
 *  - "open"          : ()
 *  - "close"         : ()
 *  - "error"         : (Error)
 *  - "response"      : (OutboundMessage)
 *  - "stream"        : (StreamEvent)
 *  - "status_change" : ("connecting"|"connected"|"disconnected")
 */
export class AgentorWsClient {
  constructor(url = "ws://localhost:8080/ws") {
    this._url = url;
    this._ws = null;
    this._listeners = {};
    this._reconnectDelay = 2000;
    this._reconnectTimer = null;
    this._intentionalClose = false;
    this.status = "disconnected";
  }

  /** Register an event listener. Returns a deregistration function. */
  on(event, fn) {
    if (!this._listeners[event]) this._listeners[event] = [];
    this._listeners[event].push(fn);
    return () => this.off(event, fn);
  }

  off(event, fn) {
    if (!this._listeners[event]) return;
    this._listeners[event] = this._listeners[event].filter((f) => f !== fn);
  }

  _emit(event, payload) {
    (this._listeners[event] || []).forEach((fn) => {
      try { fn(payload); } catch (e) { console.error("Listener error:", e); }
    });
  }

  /** Open (or re-open) the WebSocket connection. */
  connect() {
    if (this._ws && this._ws.readyState === WebSocket.OPEN) return;
    this._intentionalClose = false;
    this._setStatus("connecting");

    try {
      this._ws = new WebSocket(this._url);
    } catch (e) {
      this._emit("error", e);
      this._setStatus("disconnected");
      this._scheduleReconnect();
      return;
    }

    this._ws.addEventListener("open", () => {
      this._setStatus("connected");
      this._emit("open");
      this._reconnectDelay = 2000;
    });

    this._ws.addEventListener("close", () => {
      this._setStatus("disconnected");
      this._emit("close");
      if (!this._intentionalClose) this._scheduleReconnect();
    });

    this._ws.addEventListener("error", (e) => {
      this._emit("error", e);
    });

    this._ws.addEventListener("message", (ev) => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch { return; }
      if (msg.msg_type === "stream") {
        this._emit("stream", msg);
      } else {
        this._emit("response", msg);
      }
    });
  }

  /** Close the connection permanently (no reconnect). */
  disconnect() {
    this._intentionalClose = true;
    if (this._reconnectTimer) { clearTimeout(this._reconnectTimer); this._reconnectTimer = null; }
    if (this._ws) this._ws.close();
  }

  /** Send a message to the gateway. Returns true if sent. */
  send(content, sessionId = null) {
    if (!this._ws || this._ws.readyState !== WebSocket.OPEN) return false;
    const payload = { content };
    if (sessionId) payload.session_id = sessionId;
    this._ws.send(JSON.stringify(payload));
    return true;
  }

  _setStatus(s) {
    this.status = s;
    this._emit("status_change", s);
  }

  _scheduleReconnect() {
    if (this._reconnectTimer) return;
    this._reconnectTimer = setTimeout(() => {
      this._reconnectTimer = null;
      this._reconnectDelay = Math.min(this._reconnectDelay * 1.5, 30000);
      this.connect();
    }, this._reconnectDelay);
  }
}
