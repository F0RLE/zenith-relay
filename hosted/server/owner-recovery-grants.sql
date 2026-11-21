GRANT USAGE ON SCHEMA relay_hosted TO :"recovery_role";
GRANT SELECT ON relay_hosted.closed_installation, relay_hosted._sqlx_migrations TO :"recovery_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_recovery_context(uuid,text,uuid),
    relay_hosted.replace_owner_passkey(uuid,text,uuid,bigint,uuid,bigint,bigint,bytea,jsonb,bigint)
    TO :"recovery_role";
