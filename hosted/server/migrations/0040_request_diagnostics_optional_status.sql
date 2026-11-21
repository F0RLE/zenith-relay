-- Transport, delivery and storage observations do not carry an HTTP status.
ALTER TABLE relay_hosted.request_diagnostics
    DROP CONSTRAINT request_diagnostics_check,
    ADD CONSTRAINT request_diagnostics_http_status CHECK (
        http_status IS NULL OR (code='upstream_status' AND http_status BETWEEN 100 AND 599)
    );
