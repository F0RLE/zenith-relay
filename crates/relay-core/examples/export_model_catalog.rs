use std::{error::Error, fs, io, path::PathBuf};
use zenith_relay_core::model_metadata::{
    validate_model_order_catalog, ModelMetadataCatalogLoader, MODEL_CATALOG_SCHEMA,
    MODEL_ORDER_JSON,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let output = arguments.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../output/model-catalog")
    });
    if arguments.next().is_some() {
        return Err(io::Error::other("usage: export_model_catalog [output-directory]").into());
    }
    validate_model_order_catalog().map_err(io::Error::other)?;
    fs::create_dir_all(&output)?;
    let loader = ModelMetadataCatalogLoader::open(output.join(".cache/metadata.json"))?;
    loader.refresh(true).await?;
    loader
        .snapshot()
        .write_reference_catalog(&output.join("model-catalog.json"))?;
    fs::write(output.join("model-order.json"), MODEL_ORDER_JSON)?;
    fs::write(
        output.join("model-order.schema.json"),
        include_str!("../../../catalog/model-order.schema.json"),
    )?;
    fs::write(
        output.join("model-catalog.schema.json"),
        MODEL_CATALOG_SCHEMA,
    )?;
    Ok(())
}
