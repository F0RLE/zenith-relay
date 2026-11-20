GRANT USAGE ON SCHEMA relay_hosted TO :"vault_role";
GRANT SELECT ON relay_hosted.closed_installation, relay_hosted._sqlx_migrations TO :"vault_role";
GRANT SELECT (id, singleton, public_origin) ON relay_hosted.installations TO :"vault_role";
GRANT SELECT, INSERT ON relay_hosted.vault_bindings, relay_hosted.connection_secret_slots TO :"vault_role";
GRANT UPDATE (revision, generation, manifest_digest) ON relay_hosted.connection_secret_slots TO :"vault_role";
GRANT SELECT, INSERT ON relay_hosted.static_source_provisions TO :"vault_role";
GRANT EXECUTE ON FUNCTION relay_hosted.source_copy_access(text,uuid,uuid,uuid,bytea,boolean),
    relay_hosted.finish_source_copy(text,uuid,uuid,uuid,bytea),
    relay_hosted.source_copy_route(uuid,uuid,text,boolean) TO :"vault_role";
