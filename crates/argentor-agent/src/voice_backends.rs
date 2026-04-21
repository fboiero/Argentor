//! Voice backends — STT (Whisper, Deepgram) and TTS (OpenAI TTS,
//! ElevenLabs).
//!
//! Each backend exposes a `build_*_payload()` helper that produces the
//! provider-specific JSON body for a request. This is pure JSON
//! construction — no HTTP is performed — so the payload builders can be
//! unit tested and reused by higher-level clients.
//!
//! The [`SttBackend::transcribe`] and [`TtsBackend::synthesize`]
//! implementations currently return stubs. Real HTTP wiring can be added
//! behind a feature flag without changing the public surface.

use crate::voice::{
    AudioFormat, AudioInput, SttBackend, TranscriptionRequest, TranscriptionResult, TtsBackend,
    VoiceConfig,
};
use argentor_core::ArgentorResult;
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// OpenAI Whisper — STT
// ---------------------------------------------------------------------------

/// OpenAI Whisper (`/v1/audio/transcriptions`) STT backend.
///
/// The real endpoint takes `multipart/form-data` with the audio file plus
/// string fields. `build_request_payload` returns the equivalent JSON
/// representation that callers can either serialize into form fields or
/// send to a proxy that accepts JSON.
pub struct OpenAiWhisperBackend {
    api_key: String,
    model: String,
    api_base_url: String,
}

impl OpenAiWhisperBackend {
    /// Default Whisper model identifier.
    pub const DEFAULT_MODEL: &'static str = "whisper-1";

    /// Construct a new Whisper backend using `whisper-1`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.to_string(),
            api_base_url: "https://api.openai.com".to_string(),
        }
    }

    /// Override the model identifier (e.g. `"whisper-1"`).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the API base URL (useful for proxies or testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Build the JSON representation of the multipart form fields for the
    /// Whisper transcription endpoint.
    ///
    /// Shape:
    /// ```json
    /// {
    ///   "model": "whisper-1",
    ///   "audio": { "kind": "file_path", "value": "/tmp/a.wav" },
    ///   "language": "en",
    ///   "prompt": "Medical vocabulary.",
    ///   "response_format": "verbose_json"
    /// }
    /// ```
    pub fn build_request_payload(&self, request: &TranscriptionRequest) -> Value {
        let mut payload = json!({
            "model": self.model,
            "audio": request.audio,
            "response_format": "verbose_json",
        });

        if let Some(lang) = &request.language {
            payload["language"] = Value::String(lang.clone());
        }
        if let Some(prompt) = &request.prompt {
            payload["prompt"] = Value::String(prompt.clone());
        }

        payload
    }
}

#[async_trait]
impl SttBackend for OpenAiWhisperBackend {
    fn provider_name(&self) -> &str {
        "openai-whisper"
    }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        vec![
            AudioFormat::Wav,
            AudioFormat::Mp3,
            AudioFormat::Ogg,
            AudioFormat::Flac,
            AudioFormat::Webm,
            AudioFormat::M4a,
        ]
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> ArgentorResult<TranscriptionResult> {
        Ok(TranscriptionResult {
            text: "[stt-stub] would transcribe audio".to_string(),
            language_detected: request.language,
            duration_seconds: None,
            segments: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Deepgram — STT
// ---------------------------------------------------------------------------

/// Deepgram (`/v1/listen`) STT backend.
///
/// Deepgram accepts query parameters for configuration and raw bytes (or
/// a URL object) for the audio payload. `build_request_payload` returns
/// a JSON object with both the query params (`params`) and the body
/// shape (`body`).
pub struct DeepgramSttBackend {
    api_key: String,
    model: String,
    api_base_url: String,
}

impl DeepgramSttBackend {
    /// Default Deepgram model identifier.
    pub const DEFAULT_MODEL: &'static str = "nova-2";

    /// Construct a new Deepgram backend using `nova-2`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.to_string(),
            api_base_url: "https://api.deepgram.com".to_string(),
        }
    }

    /// Override the model identifier (e.g. `"nova-2"`, `"nova-3"`).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Build the Deepgram request payload.
    ///
    /// Shape:
    /// ```json
    /// {
    ///   "params": {
    ///     "model": "nova-2",
    ///     "language": "en",
    ///     "punctuate": true,
    ///     "smart_format": true
    ///   },
    ///   "body": { "url": "https://..." }
    /// }
    /// ```
    ///
    /// When the audio is a URL, `body` becomes `{"url": "..."}` which is
    /// exactly what Deepgram expects as JSON body. For file / base64, the
    /// caller should send raw bytes — `body` then carries a reference
    /// object with the original `AudioInput` so callers can re-extract it.
    pub fn build_request_payload(&self, request: &TranscriptionRequest) -> Value {
        let mut params = json!({
            "model": self.model,
            "punctuate": true,
            "smart_format": true,
        });

        if let Some(lang) = &request.language {
            params["language"] = Value::String(lang.clone());
        } else {
            params["detect_language"] = Value::Bool(true);
        }

        let body = match &request.audio {
            AudioInput::Url(url) => json!({ "url": url }),
            other => json!({ "audio": other }),
        };

        let mut payload = json!({
            "params": params,
            "body": body,
        });

        if let Some(prompt) = &request.prompt {
            payload["params"]["keywords"] = Value::String(prompt.clone());
        }

        payload
    }
}

#[async_trait]
impl SttBackend for DeepgramSttBackend {
    fn provider_name(&self) -> &str {
        "deepgram"
    }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        vec![
            AudioFormat::Wav,
            AudioFormat::Mp3,
            AudioFormat::Ogg,
            AudioFormat::Flac,
            AudioFormat::Webm,
            AudioFormat::M4a,
        ]
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> ArgentorResult<TranscriptionResult> {
        Ok(TranscriptionResult {
            text: "[stt-stub] would transcribe audio".to_string(),
            language_detected: request.language,
            duration_seconds: None,
            segments: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// OpenAI TTS
// ---------------------------------------------------------------------------

/// OpenAI TTS (`/v1/audio/speech`) backend.
///
/// Accepts `model`, `input`, `voice`, `response_format`, `speed`. Supported
/// voices: `alloy`, `echo`, `fable`, `onyx`, `nova`, `shimmer`.
pub struct OpenAiTtsBackend {
    api_key: String,
    model: String,
    api_base_url: String,
}

impl OpenAiTtsBackend {
    /// Default TTS model identifier.
    pub const DEFAULT_MODEL: &'static str = "tts-1";

    /// Canonical OpenAI voice identifiers.
    pub const AVAILABLE_VOICES: &'static [&'static str] =
        &["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

    /// Construct a new OpenAI TTS backend using `tts-1`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.to_string(),
            api_base_url: "https://api.openai.com".to_string(),
        }
    }

    /// Override the model identifier (e.g. `"tts-1"`, `"tts-1-hd"`).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Map an [`AudioFormat`] to OpenAI's `response_format` string.
    fn response_format_for(fmt: AudioFormat) -> &'static str {
        match fmt {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
            AudioFormat::Ogg => "opus",
            AudioFormat::Webm => "opus",
            AudioFormat::M4a => "aac",
        }
    }

    /// Build the JSON body for the `/v1/audio/speech` endpoint.
    ///
    /// Shape:
    /// ```json
    /// {
    ///   "model": "tts-1",
    ///   "input": "Hello world",
    ///   "voice": "alloy",
    ///   "response_format": "mp3",
    ///   "speed": 1.0
    /// }
    /// ```
    pub fn build_request_payload(&self, text: &str, config: &VoiceConfig) -> Value {
        json!({
            "model": self.model,
            "input": text,
            "voice": config.voice_id,
            "response_format": Self::response_format_for(config.format),
            "speed": config.speed,
        })
    }
}

#[async_trait]
impl TtsBackend for OpenAiTtsBackend {
    fn provider_name(&self) -> &str {
        "openai-tts"
    }

    fn available_voices(&self) -> Vec<String> {
        Self::AVAILABLE_VOICES
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }

    async fn synthesize(&self, _text: &str, _config: &VoiceConfig) -> ArgentorResult<Vec<u8>> {
        Ok(b"WAV-STUB-BYTES".to_vec())
    }
}

// ---------------------------------------------------------------------------
// ElevenLabs TTS
// ---------------------------------------------------------------------------

/// ElevenLabs (`/v1/text-to-speech/{voice_id}`) TTS backend.
///
/// Supports multilingual models (`eleven_multilingual_v2`, `eleven_turbo_v2`,
/// etc.) and voice settings: `stability`, `similarity_boost`, `style`,
/// `use_speaker_boost`.
pub struct ElevenLabsTtsBackend {
    api_key: String,
    model: String,
    api_base_url: String,
}

impl ElevenLabsTtsBackend {
    /// Default ElevenLabs model identifier.
    pub const DEFAULT_MODEL: &'static str = "eleven_multilingual_v2";

    /// Construct a new ElevenLabs backend.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.to_string(),
            api_base_url: "https://api.elevenlabs.io".to_string(),
        }
    }

    /// Override the model identifier.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Configured API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Configured API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Map an [`AudioFormat`] to ElevenLabs' `output_format` query value.
    fn output_format_for(fmt: AudioFormat) -> &'static str {
        match fmt {
            AudioFormat::Mp3 => "mp3_44100_128",
            AudioFormat::Wav => "pcm_44100",
            AudioFormat::Ogg => "ogg_44100",
            AudioFormat::Flac => "flac_44100",
            AudioFormat::Webm => "webm_opus_48000",
            AudioFormat::M4a => "mp3_44100_128",
        }
    }

    /// Build the JSON body for ElevenLabs TTS.
    ///
    /// Shape:
    /// ```json
    /// {
    ///   "text": "Hello",
    ///   "model_id": "eleven_multilingual_v2",
    ///   "voice_settings": {
    ///     "stability": 0.5,
    ///     "similarity_boost": 0.75,
    ///     "style": 0.0,
    ///     "use_speaker_boost": true
    ///   },
    ///   "output_format": "mp3_44100_128",
    ///   "language_code": "en"
    /// }
    /// ```
    pub fn build_request_payload(&self, text: &str, config: &VoiceConfig) -> Value {
        // Map VoiceConfig.pitch in [-1, 1] into ElevenLabs `style` in
        // [0, 1] — negative pitch becomes low style, positive maps to
        // higher style. This is a pragmatic mapping; not all fields line
        // up 1:1.
        let style = ((config.pitch + 1.0) / 2.0).clamp(0.0, 1.0);

        json!({
            "text": text,
            "model_id": self.model,
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.75,
                "style": style,
                "use_speaker_boost": true,
            },
            "output_format": Self::output_format_for(config.format),
            "language_code": config.language,
        })
    }
}

#[async_trait]
impl TtsBackend for ElevenLabsTtsBackend {
    fn provider_name(&self) -> &str {
        "elevenlabs"
    }

    fn available_voices(&self) -> Vec<String> {
        // ElevenLabs voices are account-bound and fetched via their
        // `/v1/voices` endpoint. The stub returns the canonical preset
        // voice IDs that every free-tier account receives.
        vec![
            "21m00Tcm4TlvDq8ikWAM".to_string(), // Rachel
            "29vD33N1CtxCmqQRPOHJ".to_string(), // Drew
            "2EiwWnXFnvU5JabPnv8n".to_string(), // Clyde
            "AZnzlk1XvdvUeBnXmlld".to_string(), // Domi
            "EXAVITQu4vr4xnSDxMaL".to_string(), // Bella
        ]
    }

    async fn synthesize(&self, _text: &str, _config: &VoiceConfig) -> ArgentorResult<Vec<u8>> {
        Ok(b"WAV-STUB-BYTES".to_vec())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::{AudioInput, TranscriptionRequest};

    // ============ OpenAI Whisper ============

    #[test]
    fn whisper_constructor_defaults() {
        let b = OpenAiWhisperBackend::new("sk-abc");
        assert_eq!(b.api_key(), "sk-abc");
        assert_eq!(b.model(), "whisper-1");
        assert_eq!(b.api_base_url(), "https://api.openai.com");
    }

    #[test]
    fn whisper_with_model() {
        let b = OpenAiWhisperBackend::new("k").with_model("whisper-2");
        assert_eq!(b.model(), "whisper-2");
    }

    #[test]
    fn whisper_with_base_url() {
        let b = OpenAiWhisperBackend::new("k").with_base_url("http://localhost:9000");
        assert_eq!(b.api_base_url(), "http://localhost:9000");
    }

    #[test]
    fn whisper_provider_name() {
        let b = OpenAiWhisperBackend::new("k");
        assert_eq!(b.provider_name(), "openai-whisper");
    }

    #[test]
    fn whisper_supported_formats_contains_all() {
        let b = OpenAiWhisperBackend::new("k");
        let fmts = b.supported_formats();
        for f in [
            AudioFormat::Wav,
            AudioFormat::Mp3,
            AudioFormat::Ogg,
            AudioFormat::Flac,
            AudioFormat::Webm,
            AudioFormat::M4a,
        ] {
            assert!(fmts.contains(&f), "missing {:?}", f);
        }
    }

    #[test]
    fn whisper_payload_minimal() {
        let b = OpenAiWhisperBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::FilePath("/tmp/a.wav".into()));
        let p = b.build_request_payload(&req);
        assert_eq!(p["model"], "whisper-1");
        assert_eq!(p["response_format"], "verbose_json");
        assert!(p.get("language").is_none());
        assert!(p.get("prompt").is_none());
    }

    #[test]
    fn whisper_payload_with_language() {
        let b = OpenAiWhisperBackend::new("k");
        let req =
            TranscriptionRequest::new(AudioInput::Url("https://x".into())).with_language("es");
        let p = b.build_request_payload(&req);
        assert_eq!(p["language"], "es");
    }

    #[test]
    fn whisper_payload_with_prompt() {
        let b = OpenAiWhisperBackend::new("k");
        let req =
            TranscriptionRequest::new(AudioInput::Url("https://x".into())).with_prompt("Medical.");
        let p = b.build_request_payload(&req);
        assert_eq!(p["prompt"], "Medical.");
    }

    #[test]
    fn whisper_payload_embeds_audio() {
        let b = OpenAiWhisperBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Base64 {
            format: AudioFormat::Mp3,
            data: "AAAA".into(),
        });
        let p = b.build_request_payload(&req);
        assert_eq!(p["audio"]["kind"], "base64");
        assert_eq!(p["audio"]["value"]["format"], "mp3");
        assert_eq!(p["audio"]["value"]["data"], "AAAA");
    }

    #[test]
    fn whisper_payload_custom_model_reflected() {
        let b = OpenAiWhisperBackend::new("k").with_model("whisper-xxl");
        let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()));
        let p = b.build_request_payload(&req);
        assert_eq!(p["model"], "whisper-xxl");
    }

    #[tokio::test]
    async fn whisper_transcribe_stub_returns_ok() {
        let b = OpenAiWhisperBackend::new("k");
        let req =
            TranscriptionRequest::new(AudioInput::Url("https://x".into())).with_language("en");
        let res = b.transcribe(req).await.unwrap();
        assert!(res.text.contains("stt-stub"));
        assert_eq!(res.language_detected.as_deref(), Some("en"));
    }

    // ============ Deepgram ============

    #[test]
    fn deepgram_constructor_defaults() {
        let b = DeepgramSttBackend::new("dg-key");
        assert_eq!(b.api_key(), "dg-key");
        assert_eq!(b.model(), "nova-2");
        assert_eq!(b.api_base_url(), "https://api.deepgram.com");
    }

    #[test]
    fn deepgram_with_model() {
        let b = DeepgramSttBackend::new("k").with_model("nova-3");
        assert_eq!(b.model(), "nova-3");
    }

    #[test]
    fn deepgram_with_base_url() {
        let b = DeepgramSttBackend::new("k").with_base_url("http://local");
        assert_eq!(b.api_base_url(), "http://local");
    }

    #[test]
    fn deepgram_provider_name() {
        let b = DeepgramSttBackend::new("k");
        assert_eq!(b.provider_name(), "deepgram");
    }

    #[test]
    fn deepgram_supported_formats_not_empty() {
        assert!(!DeepgramSttBackend::new("k").supported_formats().is_empty());
    }

    #[test]
    fn deepgram_payload_url_body() {
        let b = DeepgramSttBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Url("https://a.mp3".into()));
        let p = b.build_request_payload(&req);
        assert_eq!(p["body"]["url"], "https://a.mp3");
        assert_eq!(p["params"]["model"], "nova-2");
    }

    #[test]
    fn deepgram_payload_language_hint_disables_detection() {
        let b = DeepgramSttBackend::new("k");
        let req =
            TranscriptionRequest::new(AudioInput::Url("https://x".into())).with_language("es");
        let p = b.build_request_payload(&req);
        assert_eq!(p["params"]["language"], "es");
        assert!(p["params"].get("detect_language").is_none());
    }

    #[test]
    fn deepgram_payload_no_language_enables_detection() {
        let b = DeepgramSttBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()));
        let p = b.build_request_payload(&req);
        assert_eq!(p["params"]["detect_language"], true);
        assert!(p["params"].get("language").is_none());
    }

    #[test]
    fn deepgram_payload_has_default_params() {
        let b = DeepgramSttBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()));
        let p = b.build_request_payload(&req);
        assert_eq!(p["params"]["punctuate"], true);
        assert_eq!(p["params"]["smart_format"], true);
    }

    #[test]
    fn deepgram_payload_base64_body() {
        let b = DeepgramSttBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Base64 {
            format: AudioFormat::Wav,
            data: "QUFB".into(),
        });
        let p = b.build_request_payload(&req);
        assert_eq!(p["body"]["audio"]["kind"], "base64");
        assert_eq!(p["body"]["audio"]["value"]["format"], "wav");
    }

    #[test]
    fn deepgram_payload_prompt_becomes_keywords() {
        let b = DeepgramSttBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()))
            .with_prompt("argentor rust");
        let p = b.build_request_payload(&req);
        assert_eq!(p["params"]["keywords"], "argentor rust");
    }

    #[test]
    fn deepgram_payload_model_reflects_override() {
        let b = DeepgramSttBackend::new("k").with_model("nova-3");
        let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()));
        let p = b.build_request_payload(&req);
        assert_eq!(p["params"]["model"], "nova-3");
    }

    #[tokio::test]
    async fn deepgram_transcribe_stub_returns_ok() {
        let b = DeepgramSttBackend::new("k");
        let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()));
        let res = b.transcribe(req).await.unwrap();
        assert!(res.text.contains("stt-stub"));
    }

    // ============ OpenAI TTS ============

    #[test]
    fn openai_tts_constructor_defaults() {
        let b = OpenAiTtsBackend::new("sk-tts");
        assert_eq!(b.api_key(), "sk-tts");
        assert_eq!(b.model(), "tts-1");
        assert_eq!(b.api_base_url(), "https://api.openai.com");
    }

    #[test]
    fn openai_tts_with_model_hd() {
        let b = OpenAiTtsBackend::new("k").with_model("tts-1-hd");
        assert_eq!(b.model(), "tts-1-hd");
    }

    #[test]
    fn openai_tts_with_base_url() {
        let b = OpenAiTtsBackend::new("k").with_base_url("http://x");
        assert_eq!(b.api_base_url(), "http://x");
    }

    #[test]
    fn openai_tts_provider_name() {
        let b = OpenAiTtsBackend::new("k");
        assert_eq!(b.provider_name(), "openai-tts");
    }

    #[test]
    fn openai_tts_available_voices_contains_alloy() {
        let b = OpenAiTtsBackend::new("k");
        let voices = b.available_voices();
        assert!(voices.iter().any(|v| v == "alloy"));
        assert!(voices.iter().any(|v| v == "nova"));
        assert_eq!(voices.len(), 6);
    }

    #[test]
    fn openai_tts_payload_basic_shape() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy");
        let p = b.build_request_payload("hello", &cfg);
        assert_eq!(p["model"], "tts-1");
        assert_eq!(p["input"], "hello");
        assert_eq!(p["voice"], "alloy");
        assert_eq!(p["response_format"], "mp3");
        assert_eq!(p["speed"], 1.0);
    }

    #[test]
    fn openai_tts_payload_speed_reflected() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy").with_speed(1.5);
        let p = b.build_request_payload("hi", &cfg);
        assert_eq!(p["speed"], 1.5);
    }

    #[test]
    fn openai_tts_payload_format_mp3() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy").with_format(AudioFormat::Mp3);
        let p = b.build_request_payload("hi", &cfg);
        assert_eq!(p["response_format"], "mp3");
    }

    #[test]
    fn openai_tts_payload_format_wav() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy").with_format(AudioFormat::Wav);
        let p = b.build_request_payload("hi", &cfg);
        assert_eq!(p["response_format"], "wav");
    }

    #[test]
    fn openai_tts_payload_format_flac() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy").with_format(AudioFormat::Flac);
        let p = b.build_request_payload("hi", &cfg);
        assert_eq!(p["response_format"], "flac");
    }

    #[test]
    fn openai_tts_payload_ogg_and_webm_become_opus() {
        let b = OpenAiTtsBackend::new("k");
        let cfg_ogg = VoiceConfig::new("alloy").with_format(AudioFormat::Ogg);
        let cfg_webm = VoiceConfig::new("alloy").with_format(AudioFormat::Webm);
        assert_eq!(
            b.build_request_payload("x", &cfg_ogg)["response_format"],
            "opus"
        );
        assert_eq!(
            b.build_request_payload("x", &cfg_webm)["response_format"],
            "opus"
        );
    }

    #[test]
    fn openai_tts_payload_m4a_becomes_aac() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy").with_format(AudioFormat::M4a);
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["response_format"], "aac");
    }

    #[test]
    fn openai_tts_payload_model_override() {
        let b = OpenAiTtsBackend::new("k").with_model("tts-1-hd");
        let cfg = VoiceConfig::new("alloy");
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["model"], "tts-1-hd");
    }

    #[tokio::test]
    async fn openai_tts_synthesize_returns_stub_bytes() {
        let b = OpenAiTtsBackend::new("k");
        let cfg = VoiceConfig::new("alloy");
        let bytes = b.synthesize("hello world", &cfg).await.unwrap();
        assert_eq!(bytes, b"WAV-STUB-BYTES");
    }

    // ============ ElevenLabs ============

    #[test]
    fn elevenlabs_constructor_defaults() {
        let b = ElevenLabsTtsBackend::new("xi-key");
        assert_eq!(b.api_key(), "xi-key");
        assert_eq!(b.model(), "eleven_multilingual_v2");
        assert_eq!(b.api_base_url(), "https://api.elevenlabs.io");
    }

    #[test]
    fn elevenlabs_with_model_turbo() {
        let b = ElevenLabsTtsBackend::new("k").with_model("eleven_turbo_v2");
        assert_eq!(b.model(), "eleven_turbo_v2");
    }

    #[test]
    fn elevenlabs_with_base_url() {
        let b = ElevenLabsTtsBackend::new("k").with_base_url("http://x");
        assert_eq!(b.api_base_url(), "http://x");
    }

    #[test]
    fn elevenlabs_provider_name() {
        let b = ElevenLabsTtsBackend::new("k");
        assert_eq!(b.provider_name(), "elevenlabs");
    }

    #[test]
    fn elevenlabs_available_voices_preset_ids() {
        let b = ElevenLabsTtsBackend::new("k");
        let voices = b.available_voices();
        assert!(!voices.is_empty());
        // Rachel preset voice ID
        assert!(voices.iter().any(|v| v == "21m00Tcm4TlvDq8ikWAM"));
    }

    #[test]
    fn elevenlabs_payload_shape() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("rachel-id").with_language("es");
        let p = b.build_request_payload("hola", &cfg);
        assert_eq!(p["text"], "hola");
        assert_eq!(p["model_id"], "eleven_multilingual_v2");
        assert_eq!(p["language_code"], "es");
        assert!(p["voice_settings"].is_object());
        assert_eq!(p["voice_settings"]["stability"], 0.5);
        assert_eq!(p["voice_settings"]["similarity_boost"], 0.75);
        assert_eq!(p["voice_settings"]["use_speaker_boost"], true);
    }

    #[test]
    fn elevenlabs_payload_model_override() {
        let b = ElevenLabsTtsBackend::new("k").with_model("eleven_turbo_v2");
        let cfg = VoiceConfig::new("v");
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["model_id"], "eleven_turbo_v2");
    }

    #[test]
    fn elevenlabs_payload_style_from_neutral_pitch() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v"); // pitch = 0.0
        let p = b.build_request_payload("x", &cfg);
        let style = p["voice_settings"]["style"].as_f64().unwrap();
        assert!((style - 0.5).abs() < 1e-6);
    }

    #[test]
    fn elevenlabs_payload_style_from_high_pitch() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_pitch(1.0);
        let p = b.build_request_payload("x", &cfg);
        let style = p["voice_settings"]["style"].as_f64().unwrap();
        assert!((style - 1.0).abs() < 1e-6);
    }

    #[test]
    fn elevenlabs_payload_style_from_low_pitch() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_pitch(-1.0);
        let p = b.build_request_payload("x", &cfg);
        let style = p["voice_settings"]["style"].as_f64().unwrap();
        assert!(style.abs() < 1e-6);
    }

    #[test]
    fn elevenlabs_payload_format_mp3() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_format(AudioFormat::Mp3);
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["output_format"], "mp3_44100_128");
    }

    #[test]
    fn elevenlabs_payload_format_wav() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_format(AudioFormat::Wav);
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["output_format"], "pcm_44100");
    }

    #[test]
    fn elevenlabs_payload_format_ogg() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_format(AudioFormat::Ogg);
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["output_format"], "ogg_44100");
    }

    #[test]
    fn elevenlabs_payload_format_flac() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_format(AudioFormat::Flac);
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["output_format"], "flac_44100");
    }

    #[test]
    fn elevenlabs_payload_format_webm() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("v").with_format(AudioFormat::Webm);
        let p = b.build_request_payload("x", &cfg);
        assert_eq!(p["output_format"], "webm_opus_48000");
    }

    #[tokio::test]
    async fn elevenlabs_synthesize_returns_stub_bytes() {
        let b = ElevenLabsTtsBackend::new("k");
        let cfg = VoiceConfig::new("rachel");
        let bytes = b.synthesize("hola", &cfg).await.unwrap();
        assert_eq!(bytes, b"WAV-STUB-BYTES");
    }

    // ---- Trait object dispatch ----

    #[test]
    fn stt_backends_are_object_safe() {
        let _bs: Vec<Box<dyn SttBackend>> = vec![
            Box::new(OpenAiWhisperBackend::new("k")),
            Box::new(DeepgramSttBackend::new("k")),
        ];
    }

    #[test]
    fn tts_backends_are_object_safe() {
        let _bs: Vec<Box<dyn TtsBackend>> = vec![
            Box::new(OpenAiTtsBackend::new("k")),
            Box::new(ElevenLabsTtsBackend::new("k")),
        ];
    }

    #[tokio::test]
    async fn stt_backends_dispatch_dynamically() {
        let backends: Vec<Box<dyn SttBackend>> = vec![
            Box::new(OpenAiWhisperBackend::new("k")),
            Box::new(DeepgramSttBackend::new("k")),
        ];
        for b in &backends {
            let req = TranscriptionRequest::new(AudioInput::Url("https://x".into()));
            let res = b.transcribe(req).await.unwrap();
            assert!(res.text.contains("stt-stub"));
        }
    }

    #[tokio::test]
    async fn tts_backends_dispatch_dynamically() {
        let backends: Vec<Box<dyn TtsBackend>> = vec![
            Box::new(OpenAiTtsBackend::new("k")),
            Box::new(ElevenLabsTtsBackend::new("k")),
        ];
        let cfg = VoiceConfig::default();
        for b in &backends {
            let bytes = b.synthesize("hi", &cfg).await.unwrap();
            assert_eq!(bytes, b"WAV-STUB-BYTES");
        }
    }
}
