#![allow(clippy::unwrap_used, clippy::expect_used)]
//! XcapitSFF Orchestrated Agent Demo
//!
//! End-to-end showcase of the 4 XcapitSFF agent profiles running in an
//! orchestrated pipeline:
//!
//!   Phase 1 — ticket_router classifies an incoming support ticket
//!   Phase 2 — support_responder drafts a reply based on the classification
//!   Phase 3 — sales_qualifier scores a batch of 3 leads in parallel
//!   Phase 4 — outreach_composer crafts personalized outreach for the top lead
//!
//! **No API keys needed** — scripted DemoBackend with deterministic responses.
//!
//!   cargo run -p argentor-cli --example demo_xcapitsff

use argentor_agent::backends::LlmBackend;
use argentor_agent::stream::StreamEvent;
use argentor_agent::AgentRunner;
use argentor_core::{ArgentorError, ArgentorResult, Message};
use argentor_security::{AuditLog, PermissionSet};
use argentor_session::Session;
use argentor_skills::{SkillDescriptor, SkillRegistry};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};

// ── ANSI Colors ────────────────────────────────────────────────

const BOLD: &str = "\x1b[1m";
const RST: &str = "\x1b[0m";
const GRN: &str = "\x1b[32m";
const YLW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const MAG: &str = "\x1b[35m";
const DIM: &str = "\x1b[2m";
const BLU: &str = "\x1b[34m";

// ── DemoBackend ────────────────────────────────────────────────

struct DemoBackend {
    responses: Mutex<Vec<argentor_agent::llm::LlmResponse>>,
    call_count: AtomicU32,
    name: String,
}

impl DemoBackend {
    fn new(name: &str, responses: Vec<argentor_agent::llm::LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            call_count: AtomicU32::new(0),
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl LlmBackend for DemoBackend {
    fn provider_name(&self) -> &str {
        &self.name
    }

    async fn chat(
        &self,
        _system_prompt: Option<&str>,
        _messages: &[Message],
        _tools: &[SkillDescriptor],
    ) -> ArgentorResult<argentor_agent::llm::LlmResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            Err(ArgentorError::Agent(
                "DemoBackend: no more responses".into(),
            ))
        } else {
            // Simulate LLM latency
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok(responses.remove(0))
        }
    }

    async fn chat_stream(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        tools: &[SkillDescriptor],
    ) -> ArgentorResult<(
        mpsc::Receiver<StreamEvent>,
        tokio::task::JoinHandle<ArgentorResult<argentor_agent::llm::LlmResponse>>,
    )> {
        let resp = self.chat(system_prompt, messages, tools).await?;
        let (tx, rx) = mpsc::channel(1);
        let handle = tokio::spawn(async move {
            drop(tx);
            Ok(resp)
        });
        Ok((rx, handle))
    }
}

// ── Scripted Responses ─────────────────────────────────────────

fn ticket_router_response() -> argentor_agent::llm::LlmResponse {
    argentor_agent::llm::LlmResponse::Done(
        r#"{
  "category": "technical",
  "priority": "high",
  "confidence": 0.92,
  "requires_human_review": false,
  "reasoning": "El usuario reporta un error al intentar retirar fondos de su wallet DeFi. Categoría técnica por tratarse de una funcionalidad core. Prioridad alta porque involucra fondos del usuario."
}"#
        .to_string(),
    )
}

fn support_responder_response() -> argentor_agent::llm::LlmResponse {
    argentor_agent::llm::LlmResponse::Done(
        r#"## Respuesta al Cliente

Hola Martín,

Lamento mucho el inconveniente con tu retiro. Entiendo lo importante que es para vos poder acceder a tus fondos.

Revisando tu caso, el error "INSUFFICIENT_GAS" indica que la red Ethereum está congestionada y el gas estimado no fue suficiente. Esto NO significa que perdiste fondos — tu balance está intacto.

**Pasos a seguir:**
1. Intentá el retiro nuevamente en los próximos 30 minutos (el gas suele bajar)
2. Si persiste, activá la opción "Gas Priority: High" en Configuración → DeFi → Gas
3. Si necesitás el retiro urgente, respondé este mensaje y lo procesamos manualmente

## Notas Internas
- Verificar si el gas estimator está usando valores actualizados del mempool
- Ticket relacionado: #4521 (mismo error reportado ayer por 3 usuarios)
- No requiere escalamiento — solución estándar

## Escalar: NO"#
            .to_string(),
    )
}

fn lead_qualifier_responses() -> Vec<argentor_agent::llm::LlmResponse> {
    vec![
        // Lead 1: Acme Corp
        argentor_agent::llm::LlmResponse::Done(
            r#"## Calificación — Acme Corp

| Criterio | Valor | Peso |
|----------|-------|------|
| Region | LATAM | +15 |
| C-Level | Sí (CFO) | +25 |
| Afinidad | HIGH | +20 |
| Score ICP base | 75 | — |

**Score Final: 82/100**
**Clasificación: 🔥 HOT**

**Acción recomendada:** Contacto directo inmediato. Agendar demo personalizada enfocada en ROI para CFO.
**Prioridad outreach:** P1 — dentro de 24hs
**Canal sugerido:** Email ejecutivo + follow-up LinkedIn"#
                .to_string(),
        ),
        // Lead 2: TechStart SRL
        argentor_agent::llm::LlmResponse::Done(
            r#"## Calificación — TechStart SRL

| Criterio | Valor | Peso |
|----------|-------|------|
| Region | LATAM | +15 |
| C-Level | No (Dev Lead) | +5 |
| Afinidad | MEDIUM | +10 |
| Score ICP base | 45 | — |

**Score Final: 48/100**
**Clasificación: 🟡 WARM**

**Acción recomendada:** Secuencia educativa — whitepaper + case study. Nurturing 2-3 semanas antes de propuesta.
**Prioridad outreach:** P2 — dentro de 72hs
**Canal sugerido:** Email educativo + retargeting LinkedIn"#
                .to_string(),
        ),
        // Lead 3: MegaBank
        argentor_agent::llm::LlmResponse::Done(
            r#"## Calificación — MegaBank

| Criterio | Valor | Peso |
|----------|-------|------|
| Region | Iberia | +10 |
| C-Level | Sí (CTO) | +25 |
| Afinidad | HIGH | +20 |
| Score ICP base | 88 | — |

**Score Final: 91/100**
**Clasificación: 🔥 HOT**

**Acción recomendada:** Contacto inmediato. Preparar propuesta enterprise con compliance bancario. Involucrar equipo de partnerships.
**Prioridad outreach:** P0 — HOY
**Canal sugerido:** Email C-Level + llamada directa"#
                .to_string(),
        ),
    ]
}

fn outreach_composer_response() -> argentor_agent::llm::LlmResponse {
    argentor_agent::llm::LlmResponse::Done(
        r#"## Outreach para MegaBank — CTO (Score 91, HOT)

### Mensaje Principal (Email)

**Asunto:** Gestión de activos digitales para banca — caso de éxito LATAM

Estimado/a CTO,

En Xcapit ayudamos a instituciones financieras a integrar gestión de activos digitales con los estándares de compliance que la banca exige. Nuestro framework cumple ISO 27001 y ISO 42001 de forma nativa.

¿Le interesaría una conversación de 20 minutos para explorar cómo bancos similares están adoptando DeFi de forma regulada?

Quedo a disposición.

### Variante A/B

**Variante A (ROI):** "Bancos como [nombre] redujeron un 35% los costos de custodia digital con nuestra plataforma."
**Variante B (Urgencia):** "La regulación MiCA entra en vigor en Q3 — las instituciones que se preparen ahora tendrán ventaja competitiva."

### Siguiente Paso
→ Si responde: agendar demo técnica con equipo de compliance
→ Si no responde en 3 días: follow-up LinkedIn al CTO con link al whitepaper de banca digital
→ Si no responde en 7 días: nurturing con newsletter institucional"#
            .to_string(),
    )
}

// ── Display helpers ────────────────────────────────────────────

fn section(num: u32, title: &str) {
    println!();
    println!("{BOLD}{CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RST}");
    println!("{BOLD}{CYAN}  Phase {num}: {title}{RST}");
    println!("{BOLD}{CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RST}");
}

fn agent_tag(role: &str) -> String {
    let color = match role {
        "ticket_router" => BLU,
        "support_responder" => GRN,
        "sales_qualifier" => YLW,
        "outreach_composer" => MAG,
        _ => DIM,
    };
    format!("{color}{BOLD}[{role}]{RST}")
}

fn print_response(role: &str, response: &str, duration_ms: u64) {
    let tag = agent_tag(role);
    println!();
    println!("  {tag} {DIM}({duration_ms}ms){RST}");
    println!("{DIM}  ┌─────────────────────────────────────────────{RST}");
    for line in response.lines() {
        println!("{DIM}  │{RST} {line}");
    }
    println!("{DIM}  └─────────────────────────────────────────────{RST}");
}

// ── Main ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    println!();
    println!("{BOLD}{MAG}╔═══════════════════════════════════════════════════╗{RST}");
    println!("{BOLD}{MAG}║     XcapitSFF × Argentor — Orchestrated Demo     ║{RST}");
    println!("{BOLD}{MAG}╚═══════════════════════════════════════════════════╝{RST}");
    println!();
    println!("  {DIM}4 agentes AI en pipeline orquestado{RST}");
    println!("  {DIM}Sin API keys — respuestas simuladas con DemoBackend{RST}");
    println!("  {DIM}Audit log + compliance + circuit breaker activos{RST}");

    let audit = Arc::new(AuditLog::new(std::path::PathBuf::from(
        "/tmp/argentor-demo-xcapit",
    )));
    let skills = Arc::new(SkillRegistry::new());
    let permissions = PermissionSet::new();

    let total_start = Instant::now();
    let mut total_tokens: u64 = 0;

    // ── Phase 1: Ticket Router ──────────────────────────────────

    section(1, "Ticket Router — Clasificación de ticket entrante");

    let ticket_input = "Hola, no puedo retirar mis fondos de la wallet DeFi. \
        Cuando intento hacer withdraw me sale error INSUFFICIENT_GAS y se queda cargando. \
        Ya intenté 3 veces. Mi cuenta es martin@acme.com, tengo 2.4 ETH en staking.";

    println!();
    println!("  {DIM}Ticket entrante:{RST}");
    println!("  {DIM}\"{}\"  {RST}", &ticket_input[..80]);

    let start = Instant::now();
    let backend = DemoBackend::new("claude-demo", vec![ticket_router_response()]);
    let runner = AgentRunner::from_backend(
        Box::new(backend),
        skills.clone(),
        permissions.clone(),
        audit.clone(),
        3,
    )
    .with_system_prompt(
        "Clasificás tickets de soporte por categoría y prioridad. Respondé SOLO con JSON.",
    );

    let mut session = Session::new();
    let result = runner.run(&mut session, ticket_input).await.unwrap();
    let dur = start.elapsed().as_millis() as u64;
    total_tokens += (ticket_input.len() / 4 + result.len() / 4) as u64;

    print_response("ticket_router", &result, dur);

    // ── Phase 2: Support Responder ──────────────────────────────

    section(2, "Support Responder — Respuesta al ticket clasificado");

    let support_context = format!(
        "Ticket clasificado como: technical / high (confidence: 0.92)\n\
         Mensaje original del cliente: {ticket_input}\n\n\
         Redactá una respuesta empática y accionable."
    );

    let start = Instant::now();
    let backend = DemoBackend::new("claude-demo", vec![support_responder_response()]);
    let runner = AgentRunner::from_backend(
        Box::new(backend),
        skills.clone(),
        permissions.clone(),
        audit.clone(),
        3,
    )
    .with_system_prompt("Sos el agente de soporte al cliente de Xcapit. Respondé con: respuesta al cliente, notas internas, y si hay que escalar.");

    let mut session = Session::new();
    let result = runner.run(&mut session, &support_context).await.unwrap();
    let dur = start.elapsed().as_millis() as u64;
    total_tokens += (support_context.len() / 4 + result.len() / 4) as u64;

    print_response("support_responder", &result, dur);

    // ── Phase 3: Sales Qualifier — Batch de 3 leads ─────────────

    section(3, "Sales Qualifier — Calificación paralela de 3 leads");

    let leads = [
        ("Acme Corp", "LATAM", "CFO", 75, "HIGH"),
        ("TechStart SRL", "LATAM", "Dev Lead", 45, "MEDIUM"),
        ("MegaBank", "Iberia", "CTO", 88, "HIGH"),
    ];

    let qualifier_responses = lead_qualifier_responses();
    let start = Instant::now();

    // Execute in parallel
    let mut handles = Vec::new();
    for (i, ((company, region, title, score, affinity), response)) in leads
        .iter()
        .zip(qualifier_responses.into_iter())
        .enumerate()
    {
        let skills = skills.clone();
        let permissions = permissions.clone();
        let audit = audit.clone();
        let context = format!(
            "Lead to qualify:\n  Company: {company}\n  Region: {region}\n  \
             C-Level: {}\n  Score ICP: {score}\n  Afinidad: {affinity}",
            if *title == "CFO" || *title == "CTO" {
                "true"
            } else {
                "false"
            }
        );

        println!(
            "  {DIM}[{i}] {company} — {region}, {title}, ICP {score}, Afinidad {affinity}{RST}"
        );

        let handle = tokio::spawn(async move {
            let backend = DemoBackend::new("claude-demo", vec![response]);
            let runner =
                AgentRunner::from_backend(Box::new(backend), skills, permissions, audit, 3)
                    .with_system_prompt(
                        "Evaluás leads usando ICP scoring. Clasificás como hot/warm/cool/cold.",
                    );

            let mut session = Session::new();
            let result = runner.run(&mut session, &context).await;
            (i, result, context.len())
        });
        handles.push(handle);
    }

    println!();
    println!("  {DIM}Ejecutando 3 calificaciones en paralelo...{RST}");

    let mut lead_results = Vec::new();
    for handle in handles {
        let (idx, result, ctx_len) = handle.await.unwrap();
        let response = result.unwrap();
        total_tokens += (ctx_len / 4 + response.len() / 4) as u64;
        lead_results.push((idx, response));
    }

    lead_results.sort_by_key(|(idx, _)| *idx);

    let dur = start.elapsed().as_millis() as u64;
    for (idx, response) in &lead_results {
        let company = leads[*idx].0;
        print_response(
            "sales_qualifier",
            &format!("Lead: {company}\n{response}"),
            dur / 3,
        );
    }

    println!();
    println!("  {GRN}{BOLD}✓ 3 leads calificados en {dur}ms (paralelo){RST}");

    // ── Phase 4: Outreach Composer ──────────────────────────────

    section(
        4,
        "Outreach Composer — Mensaje para lead top (MegaBank, Score 91)",
    );

    let outreach_context = "Componer mensaje de outreach para:\n\
        Company: MegaBank\n\
        Region: Iberia\n\
        Contacto: CTO\n\
        Score: 91 (HOT)\n\
        Afinidad: HIGH\n\
        Canal primario: Email\n\n\
        Contexto: Banco europeo interesado en custody de activos digitales con compliance.";

    let start = Instant::now();
    let backend = DemoBackend::new("claude-demo", vec![outreach_composer_response()]);
    let runner = AgentRunner::from_backend(
        Box::new(backend),
        skills.clone(),
        permissions.clone(),
        audit.clone(),
        3,
    )
    .with_system_prompt(
        "Componés mensajes de outreach personalizados. Incluí variante A/B y siguiente paso.",
    );

    let mut session = Session::new();
    let result = runner.run(&mut session, outreach_context).await.unwrap();
    let dur = start.elapsed().as_millis() as u64;
    total_tokens += (outreach_context.len() / 4 + result.len() / 4) as u64;

    print_response("outreach_composer", &result, dur);

    // ── Summary ─────────────────────────────────────────────────

    let total_duration = total_start.elapsed();

    println!();
    println!("{BOLD}{CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RST}");
    println!("{BOLD}{CYAN}  Resumen del Pipeline Orquestado{RST}");
    println!("{BOLD}{CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RST}");
    println!();
    println!("  {BOLD}Agentes ejecutados:{RST}  4 roles, 6 invocaciones totales");
    println!("  {BOLD}Ejecución paralela:{RST}  3 leads calificados simultáneamente");
    println!("  {BOLD}Tokens estimados:{RST}   ~{total_tokens}");
    println!(
        "  {BOLD}Duración total:{RST}     {:.1}s",
        total_duration.as_secs_f64()
    );
    println!("  {BOLD}Audit log:{RST}          /tmp/argentor-demo-xcapit/");
    println!();

    println!("  {GRN}{BOLD}Pipeline:{RST}");
    println!("  {BLU}ticket_router{RST} → {GRN}support_responder{RST}");
    println!("  {YLW}sales_qualifier{RST} ×3 (paralelo) → {MAG}outreach_composer{RST}");
    println!();
    println!("  {DIM}Argentor — Secure AI Agent Framework × XcapitSFF{RST}");
    println!();
}
