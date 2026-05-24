//! MCP prompts (paper §IV-F).
//!
//! Server-rendered prompt templates that the MCP client can instantiate
//! with arguments.  Each prompt encapsulates a SAP-specific workflow that
//! the model would otherwise have to compose from scratch.

use mcp_core::{GetPromptResult, Prompt, PromptArgument, PromptMessage, Role, ToolContent};
use mcp_server::{registry::PromptHandler, PromptDescriptor};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub fn all() -> Vec<PromptDescriptor> {
    vec![
        review_rfc_call(),
        transport_impact_analysis(),
    ]
}

fn review_rfc_call() -> PromptDescriptor {
    struct H;
    impl PromptHandler for H {
        fn get(&self, arguments: Option<serde_json::Value>) -> Pin<Box<dyn Future<Output = mcp_core::Result<GetPromptResult>> + Send + '_>> {
            let args = arguments.unwrap_or(serde_json::Value::Object(Default::default()));
            let function = args.get("function").and_then(|v| v.as_str()).unwrap_or("<UNKNOWN>").to_string();
            let parameters = args.get("parameters").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
            Box::pin(async move {
                let body = format!(
                    "Review the following proposed SAP RFC call before execution. Confirm it is the right function for the user's intent, that every required parameter is present and well-typed, that the parameter values are realistic for the target system, and that the side-effects are acceptable. Cite the source for each claim.\n\nFunction: {function}\nParameters:\n{}\n\nIf safe, summarise what the call will do, the affected tables, and the user-visible result. If unsafe, identify the specific risk and propose a safer alternative.",
                    serde_json::to_string_pretty(&parameters).unwrap_or_default(),
                );
                Ok(GetPromptResult {
                    description: Some("Pre-execution review of a proposed sap.rfc.call".into()),
                    messages: vec![PromptMessage { role: Role::User, content: ToolContent::text(body) }],
                })
            })
        }
    }
    PromptDescriptor {
        prompt: Prompt {
            name: "sap.review-rfc-call".into(),
            description: Some("Pre-flight review of a proposed sap.rfc.call invocation.".into()),
            arguments: vec![
                PromptArgument { name: "function".into(), description: Some("RFC function name".into()), required: true },
                PromptArgument { name: "parameters".into(), description: Some("Parameters object".into()), required: false },
            ],
        },
        handler: Arc::new(H),
    }
}

fn transport_impact_analysis() -> PromptDescriptor {
    struct H;
    impl PromptHandler for H {
        fn get(&self, arguments: Option<serde_json::Value>) -> Pin<Box<dyn Future<Output = mcp_core::Result<GetPromptResult>> + Send + '_>> {
            let args = arguments.unwrap_or(serde_json::Value::Object(Default::default()));
            let transport = args.get("transport").and_then(|v| v.as_str()).unwrap_or("<TRANSPORT>").to_string();
            let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("PRODUCTION").to_string();
            Box::pin(async move {
                let body = format!(
                    "Analyse the impact of SAP transport {transport} on the {scope} system.\n\nSteps:\n1. Use sap.docs.search to find any related Help Portal content for the objects in the transport.\n2. Use sap.rfc.search to find the RFCs that reference any modified ABAP objects.\n3. Use sap.table.read on E070/E071 to enumerate transport contents.\n4. Use eam.impact_map (when LeanIX is wired) to enumerate downstream applications.\n5. Produce a 3-section report: Direct impact, Indirect impact, Recommended pre-import checks.\n\nCite every claim by its source URI.",
                );
                Ok(GetPromptResult {
                    description: Some("Cross-domain impact analysis for an SAP transport".into()),
                    messages: vec![PromptMessage { role: Role::User, content: ToolContent::text(body) }],
                })
            })
        }
    }
    PromptDescriptor {
        prompt: Prompt {
            name: "sap.transport-impact-analysis".into(),
            description: Some("Multi-tool cross-domain impact analysis for an SAP transport.".into()),
            arguments: vec![
                PromptArgument { name: "transport".into(), description: Some("Transport ID (e.g. ZTRA01K900123)".into()), required: true },
                PromptArgument { name: "scope".into(), description: Some("Target system (PRODUCTION / QA / DEV)".into()), required: false },
            ],
        },
        handler: Arc::new(H),
    }
}
