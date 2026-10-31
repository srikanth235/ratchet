//! Evaluator templates: the gallery registry, rendering to files, and the "Use template" pull request.

pub mod files;
pub mod propose;
pub mod registry;
pub mod render;

pub use files::{template_files, template_ids_with_files};
pub use propose::{TemplateProposal, build_template_proposal};
pub use registry::{
    EvaluatorTemplate, TEMPLATE_ALIASES, TemplateFilter, TemplateInfo, TemplateObjective,
    evaluator_template_ids, find_evaluator_template, get_template, list_templates,
    resolve_template_id,
};
pub use render::{
    RenderTemplateOptions, RenderedTemplate, ResolvedTemplateOptions, TemplateError, TemplateFile,
    render_template, template_file_paths,
};
