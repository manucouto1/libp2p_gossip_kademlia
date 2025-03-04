FROM rust:latest

WORKDIR /usr/src/app

# Instala dependencias del sistema (ejemplo para proyectos con OpenSSL)
RUN apt-get update && \
    apt-get install -y libssl-dev pkg-config && \
    apt-get update && apt-get install -y iputils-ping net-tools curl iptables && \
    rm -rf /var/lib/apt/lists/*

# Copia y construye el proyecto
COPY . .
RUN cargo build --release
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Ejecuta el binario
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
