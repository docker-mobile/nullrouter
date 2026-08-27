use std::{
    env, io,
    net::{IpAddr, SocketAddr},
};

use actix_web::{App, HttpServer};
use nullrouter_auth::{AuthConfig, AuthService, DEFAULT_HOST, DEFAULT_PORT, configure};

#[actix_web::main]
async fn main() -> io::Result<()> {
    let listen_addr = listen_addr()?;
    let service = AuthService::from_config(
        AuthConfig::from_env()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    HttpServer::new(move || App::new().configure(configure(service.clone())))
        .bind(listen_addr)?
        .run()
        .await
}

fn listen_addr() -> io::Result<SocketAddr> {
    let host = env::var("NULLROUTER_AUTH_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_owned());
    let address = host
        .parse::<IpAddr>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid auth host"))?;
    if !address.is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "nullrouter-auth must bind to a loopback address",
        ));
    }
    let port = match env::var("NULLROUTER_AUTH_PORT") {
        Ok(value) => value
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid auth port"))?,
        Err(env::VarError::NotPresent) => DEFAULT_PORT,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid auth port",
            ));
        }
    };
    Ok(SocketAddr::new(address, port))
}
