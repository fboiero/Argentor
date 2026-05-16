"""Tests for the Argentor Python SDK client."""

from __future__ import annotations

import json
from unittest.mock import MagicMock, patch

import httpx
import pytest

import argentor
from argentor import ArgentorClient, AsyncArgentorClient
from argentor.client import _DEFAULT_BASE_URL, _build_headers
from argentor.exceptions import (
    ArgentorAPIError,
    ArgentorConnectionError,
    ArgentorError,
    ArgentorTimeoutError,
)


# ---------------------------------------------------------------------------
# Version
# ---------------------------------------------------------------------------


class TestVersion:
    def test_version_is_set(self):
        assert argentor.__version__ is not None
        assert isinstance(argentor.__version__, str)
        assert argentor.__version__ == "1.4.7"

    def test_version_follows_semver(self):
        parts = argentor.__version__.split(".")
        assert len(parts) == 3
        for part in parts:
            assert part.isdigit()


# ---------------------------------------------------------------------------
# Header building
# ---------------------------------------------------------------------------


class TestBuildHeaders:
    def test_default_headers_contain_content_type(self):
        headers = _build_headers(None, None)
        assert headers["Content-Type"] == "application/json"

    def test_api_key_header(self):
        headers = _build_headers("sk-test", None)
        assert headers["X-API-Key"] == "sk-test"

    def test_tenant_id_header(self):
        headers = _build_headers(None, "tenant-42")
        assert headers["X-Tenant-ID"] == "tenant-42"

    def test_both_headers(self):
        headers = _build_headers("sk-test", "tenant-42")
        assert headers["X-API-Key"] == "sk-test"
        assert headers["X-Tenant-ID"] == "tenant-42"

    def test_no_api_key_when_none(self):
        headers = _build_headers(None, None)
        assert "X-API-Key" not in headers

    def test_no_tenant_id_when_none(self):
        headers = _build_headers(None, None)
        assert "X-Tenant-ID" not in headers


# ---------------------------------------------------------------------------
# Client instantiation
# ---------------------------------------------------------------------------


class TestClientInstantiation:
    def test_default_base_url(self):
        client = ArgentorClient()
        assert client.base_url == "http://localhost:8080"
        client.close()

    def test_custom_base_url(self):
        client = ArgentorClient(base_url="http://custom:9090")
        assert client.base_url == "http://custom:9090"
        client.close()

    def test_trailing_slash_stripped(self):
        client = ArgentorClient(base_url="http://localhost:8080/")
        assert client.base_url == "http://localhost:8080"
        client.close()

    def test_context_manager(self):
        with ArgentorClient() as client:
            assert client.base_url == _DEFAULT_BASE_URL


class TestAsyncClientInstantiation:
    def test_async_client_default_url(self):
        client = AsyncArgentorClient()
        assert client.base_url == "http://localhost:8080"

    def test_async_client_custom_url(self):
        client = AsyncArgentorClient(base_url="http://custom:9090")
        assert client.base_url == "http://custom:9090"


# ---------------------------------------------------------------------------
# URL building for endpoints
# ---------------------------------------------------------------------------


class TestUrlBuilding:
    def test_health_endpoint(self):
        client = ArgentorClient(base_url="http://api.example.com")
        assert client._http.base_url == httpx.URL("http://api.example.com")
        client.close()

    def test_base_url_used_by_http_client(self):
        client = ArgentorClient(base_url="https://my-server:3000")
        assert "my-server" in str(client._http.base_url)
        client.close()


# ---------------------------------------------------------------------------
# Error handling with mocked HTTP
# ---------------------------------------------------------------------------


class TestSyncClientMethods:
    @patch("argentor.client.httpx.Client")
    def test_health_success(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = {"status": "ok", "version": "1.0.0"}
        mock_http.get.return_value = mock_resp

        client = ArgentorClient()
        result = client.health()
        assert result["status"] == "ok"

    @patch("argentor.client.httpx.Client")
    def test_run_task_sends_correct_payload(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = {"task_id": "t1", "status": "completed"}
        mock_http.post.return_value = mock_resp

        client = ArgentorClient()
        result = client.run_task(role="assistant", context="Hello")
        mock_http.post.assert_called_once()
        call_args = mock_http.post.call_args
        assert call_args[0][0] == "/v1/run"
        payload = call_args[1]["json"]
        assert payload["agent_role"] == "assistant"
        assert payload["context"] == "Hello"
        assert result["task_id"] == "t1"

    @patch("argentor.client.httpx.Client")
    def test_api_error_raised_on_4xx(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = False
        mock_resp.status_code = 404
        mock_resp.json.return_value = {"detail": "Not found"}
        mock_resp.reason_phrase = "Not Found"
        mock_http.get.return_value = mock_resp

        client = ArgentorClient()
        with pytest.raises(ArgentorAPIError) as exc_info:
            client.health()
        assert exc_info.value.status_code == 404
        assert "Not found" in exc_info.value.message

    @patch("argentor.client.httpx.Client")
    def test_list_skills(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = [{"name": "echo"}, {"name": "time"}]
        mock_http.get.return_value = mock_resp

        client = ArgentorClient()
        skills = client.list_skills()
        assert len(skills) == 2
        assert skills[0]["name"] == "echo"

    @patch("argentor.client.httpx.Client")
    def test_execute_skill(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = {"success": True, "output": "Hello!"}
        mock_http.post.return_value = mock_resp

        client = ArgentorClient()
        result = client.execute_skill("echo", {"text": "Hello!"})
        assert result["success"] is True

    @patch("argentor.client.httpx.Client")
    def test_batch_tasks(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = {
            "batch_id": "b1",
            "results": [],
            "total": 2,
            "succeeded": 2,
            "failed": 0,
        }
        mock_http.post.return_value = mock_resp

        client = ArgentorClient()
        result = client.batch_tasks(
            [{"agent_role": "a", "context": "c1"}, {"agent_role": "a", "context": "c2"}]
        )
        assert result["batch_id"] == "b1"

    @patch("argentor.client.httpx.Client")
    def test_create_session(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = {"session_id": "s-123"}
        mock_http.post.return_value = mock_resp

        client = ArgentorClient()
        result = client.create_session()
        assert result["session_id"] == "s-123"

    @patch("argentor.client.httpx.Client")
    def test_metrics_returns_text(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.text = "# HELP requests_total\nrequests_total 42"
        mock_http.get.return_value = mock_resp

        client = ArgentorClient()
        result = client.metrics()
        assert "requests_total" in result

    @patch("argentor.client.httpx.Client")
    def test_enterprise_readiness(self, mock_client_cls):
        mock_http = MagicMock()
        mock_client_cls.return_value = mock_http
        mock_resp = MagicMock()
        mock_resp.is_success = True
        mock_resp.json.return_value = {
            "version": "1.4.7",
            "posture": "ready",
            "score": 86,
            "runtime": {
                "skills_registered": 42,
                "active_connections": 0,
                "active_sessions": 0,
                "uptime_seconds": 10,
            },
            "checks": [],
            "next_actions": [],
        }
        mock_http.get.return_value = mock_resp

        client = ArgentorClient()
        result = client.enterprise_readiness()
        mock_http.get.assert_called_with("/api/v1/enterprise/readiness")
        assert result["posture"] == "ready"
        assert result["score"] == 86


# ---------------------------------------------------------------------------
# Exception hierarchy
# ---------------------------------------------------------------------------


class TestExceptions:
    def test_argentor_error_base(self):
        err = ArgentorError("test")
        assert err.message == "test"
        assert str(err) == "test"

    def test_api_error_attributes(self):
        err = ArgentorAPIError("Not found", 404, {"detail": "Not found"})
        assert err.status_code == 404
        assert err.response_body == {"detail": "Not found"}
        assert "404" in str(err)

    def test_connection_error(self):
        err = ArgentorConnectionError("refused")
        assert isinstance(err, ArgentorError)
        assert err.message == "refused"

    def test_timeout_error(self):
        err = ArgentorTimeoutError("timed out")
        assert isinstance(err, ArgentorError)
        assert err.message == "timed out"


# ---------------------------------------------------------------------------
# Exports
# ---------------------------------------------------------------------------


class TestExports:
    def test_all_exports_accessible(self):
        for name in argentor.__all__:
            assert hasattr(argentor, name), f"{name} not accessible from argentor"
