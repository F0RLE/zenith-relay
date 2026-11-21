GRANT USAGE ON SCHEMA relay_hosted TO :"qualification_role";
GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO :"qualification_role";
GRANT EXECUTE ON FUNCTION relay_hosted.text_check_scope(text),relay_hosted.text_check_step(text,uuid,uuid,uuid,bigint,bigint,boolean,jsonb,text),
    relay_hosted.text_stream_check(text,uuid,uuid,uuid,text),
    relay_hosted.record_text_check(text,uuid,uuid,uuid,bigint,bigint,boolean,text,text,text,integer,integer,integer,integer,integer,integer,inet) TO :"qualification_role";
GRANT EXECUTE ON FUNCTION relay_hosted.take_web_model_check(text),relay_hosted.step_web_model_check(text,jsonb,text),
    relay_hosted.finish_web_model_check(text,uuid) TO :"qualification_role";
