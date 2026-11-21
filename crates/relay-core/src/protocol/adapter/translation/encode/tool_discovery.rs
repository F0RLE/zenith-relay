use super::*;

/// Place each loaded definition at its first discovery, or before a retained
/// call when the discovery was compacted. Forced tools without history load at
/// the end. Initial declarations precede history, so downstream SIWC preparation
/// cannot move an unused initial tool to the tail after the first discovery.
pub(super) fn additions(request: &Request) -> AdapterResult<BTreeMap<usize, Value>> {
    let Some(deferred) = &request.deferred_tools else {
        return Ok(BTreeMap::new());
    };
    let mut pending: BTreeMap<_, _> = request
        .tools
        .iter()
        .filter(|tool| deferred.contains_key(&tool.name))
        .map(|tool| (tool.name.as_str(), tool))
        .collect();
    let mut discoveries: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, ids) in deferred {
        for id in ids {
            discoveries.entry(id).or_default().push(name);
        }
    }
    let mut boundaries: BTreeMap<usize, Vec<&Function>> = BTreeMap::new();
    let initial = request
        .tools
        .iter()
        .filter(|tool| !deferred.contains_key(&tool.name))
        .collect::<Vec<_>>();
    if !initial.is_empty() {
        boundaries.insert(0, initial);
    }
    for (index, message) in request.messages.iter().enumerate() {
        for block in &message.blocks {
            match block {
                Block::ToolCall { name, .. } => {
                    if let Some(tool) = pending.remove(name.as_str()) {
                        boundaries.entry(index).or_default().push(tool);
                    }
                }
                Block::ToolResult { id, .. } => {
                    for name in discoveries.get(id.as_str()).into_iter().flatten() {
                        if let Some(tool) = pending.remove(name) {
                            boundaries.entry(index + 1).or_default().push(tool);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if !pending.is_empty() {
        boundaries
            .entry(request.messages.len())
            .or_default()
            .extend(pending.into_values());
    }
    boundaries
        .into_iter()
        .map(|(index, tools)| {
            let tools = tools
                .into_iter()
                .map(|tool| parts::function(tool, WireApi::Responses))
                .collect::<AdapterResult<Vec<_>>>()?;
            Ok((
                index,
                json!({"type":"additional_tools","role":"developer","tools":tools}),
            ))
        })
        .collect()
}
