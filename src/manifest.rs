//! Capability manifest — what Five can do, in voice-friendly form.
//!
//! Detection only: the actual text generation lives in main.rs so it can
//! access the live HomeClient for configured devices/scenes.

/// A capability category with speakable description.
#[derive(Debug, Clone)]
pub struct Capability {
    pub name: &'static str,
    pub description: &'static str,
    pub examples: &'static [&'static str],
}

/// Static capabilities — things Five can do without external services.
pub fn static_capabilities() -> Vec<Capability> {
    vec![
        Capability {
            name: "conversation",
            description: "General questions and chat",
            examples: &[
                "what's the weather like",
                "explain quantum computing",
                "who won the game last night",
            ],
        },
        Capability {
            name: "mode_switch",
            description: "Switch brain modes",
            examples: &[
                "switch to DM mode",
                "activate deep think mode",
                "back to normal",
            ],
        },
        Capability {
            name: "context",
            description: "Clear conversation history",
            examples: &["clear context", "forget everything we talked about"],
        },
        Capability {
            name: "time",
            description: "Time and date",
            examples: &["what time is it", "what's today's date"],
        },
        Capability {
            name: "memory",
            description: "Long-term memory notebook, kept between sessions",
            examples: &["remember this", "what do you remember about me"],
        },
        Capability {
            name: "search",
            description: "Web search on whitelisted sites",
            examples: &["search the web for rust iterators", "look this up"],
        },
        Capability {
            name: "lessons",
            description: "The big model authors a lesson plan, then Five teaches it step by step",
            examples: &[
                "make a lesson plan for the french revolution",
                "start the lesson",
                "next section",
                "skip to the section on napoleon",
                "list lessons",
                "end the lesson",
            ],
        },
        Capability {
            name: "files",
            description: "Save notes and snippets to files",
            examples: &["write this down", "save as shopping list", "add to my notes"],
        },
        Capability {
            name: "conversation_mode",
            description: "Keep talking after the wake word — no need to repeat it",
            examples: &["five, what is recursion", "...(follow-ups just work)"],
        },
        Capability {
            name: "bridge",
            description: "Route voice to a live Claude Code session",
            examples: &["switch to coding mode", "back to normal"],
        },
        Capability {
            name: "help",
            description: "List capabilities",
            examples: &["help", "what can you do", "list commands"],
        },
    ]
}

/// Detect a request for the manifest/help.
pub fn wants_help(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    matches!(t.as_str(), "help" | "what can you do" | "what can you do?"
        | "list commands" | "list commands?" | "show commands"
        | "what are your commands" | "what do you know"
        | "manifest" | "capabilities")
    || t.starts_with("help me")
    || t.starts_with("what can you")
}

/// Detect a request to list devices/scenes.
pub fn wants_device_list(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t.contains("list my lights")
        || t.contains("list my devices")
        || t.contains("what lights")
        || t.contains("what devices")
        || t.contains("what scenes")
        || (t.contains("list") && t.contains("light"))
        || (t.contains("list") && t.contains("scene"))
}
