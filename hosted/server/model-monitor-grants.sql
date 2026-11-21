GRANT USAGE ON SCHEMA relay_hosted TO :"model_monitor_role";
GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO :"model_monitor_role";
GRANT EXECUTE ON FUNCTION relay_hosted.model_monitor_targets(text),
    relay_hosted.attest_model_status(text,uuid,timestamptz,text) TO :"model_monitor_role";
