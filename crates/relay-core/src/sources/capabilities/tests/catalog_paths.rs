use super::*;

#[test]
fn catalog_endpoint_paths_declare_only_the_exact_native_protocols() {
    for field in [
        "supported_endpoint_types",
        "supportedEndpointTypes",
        "supported_endpoints",
        "endpoints",
    ] {
        for (paths, available) in [
            (vec!["/api/v1/responses"], vec![WireApi::Responses]),
            (
                vec!["/api/v1/chat/completions"],
                vec![WireApi::ChatCompletions],
            ),
            (vec!["/api/v1/messages"], vec![WireApi::Messages]),
            (
                vec!["/api/v1/messages", "/api/v1/responses"],
                vec![WireApi::Messages, WireApi::Responses],
            ),
            (
                vec!["/api/v1/videos", "/api/v1/images", "/api/alpha/extractions"],
                vec![],
            ),
            (
                vec![
                    "https://other.example.test/api/v1/responses",
                    "/api/v1/responses/extra",
                ],
                vec![],
            ),
            (vec![], vec![]),
        ] {
            let mut model = json!({"id":"synthetic-model"});
            model[field] = json!(paths);
            let observations = catalog_capabilities(&json!({"data":[model]}), 42);
            assert_eq!(observations.len(), WireApi::ALL.len(), "{field}");
            for observation in observations {
                assert_eq!(
                    observation.status,
                    if available.contains(&observation.upstream_wire_api) {
                        CapabilityStatus::Declared
                    } else {
                        CapabilityStatus::Unsupported
                    },
                    "{field}: {:?}",
                    observation.upstream_wire_api
                );
                assert_eq!(observation.origin, CapabilityOrigin::Catalog);
                assert!(observation.features.is_empty());
                assert!(observation.reasoning_efforts.is_empty());
            }
        }
    }
}

#[test]
fn published_endpoint_paths_keep_pins_and_manual_exclusions_after_reload() {
    let base = "https://source.example.test/api/v1";
    let models = vec!["synthetic-model".into()];
    for (pin, excluded) in [
        (None, false),
        (Some(WireApi::ChatCompletions), false),
        (None, true),
    ] {
        let mut config = SourceProtocolConfig {
            endpoint_hint: pin,
            ..Default::default()
        };
        if excluded {
            config.capabilities.push(
                serde_json::from_value(json!({
                    "modelId":"synthetic-model","upstreamWireApi":"responses",
                    "status":"unsupported","origin":"manual","checkedAtMs":1
                }))
                .unwrap(),
            );
        }
        config.merge_catalog(catalog_capabilities(
            &json!({"data":[{
                "id":"synthetic-model","endpoints":["/api/v1/responses"]
            }]}),
            42,
        ));
        let restored: SourceProtocolConfig =
            serde_json::from_value(serde_json::to_value(config).unwrap()).unwrap();
        let routes = restored
            .resolve(base, &models, &[], WireApi::ChatCompletions)
            .unwrap();
        if pin.is_some() || excluded {
            assert!(routes.is_empty());
        } else {
            assert_eq!(
                upstreams(&routes, "synthetic-model"),
                BTreeSet::from([WireApi::Responses])
            );
            assert!(
                routes
                    .iter()
                    .filter(|route| route.wire_api == WireApi::Responses)
                    .all(|route| route.adapter == SourceAdapter::Native)
            );
        }
    }
}

#[test]
fn existing_endpoint_fields_take_precedence_over_published_path_aliases() {
    for (model, expected) in [
        (
            json!({"id":"synthetic-model","supported_endpoint_types":["messages"],"endpoints":["/api/v1/responses"]}),
            WireApi::Messages,
        ),
        (
            json!({"name":"models/synthetic-model","supportedGenerationMethods":["generateContent"],"endpoints":["/api/v1/responses"]}),
            WireApi::Gemini,
        ),
    ] {
        let observations = catalog_capabilities(&json!({"data":[model]}), 42);
        assert_eq!(
            observations
                .iter()
                .filter(|row| row.status.available())
                .map(|row| row.upstream_wire_api)
                .collect::<Vec<_>>(),
            [expected]
        );
    }
}
