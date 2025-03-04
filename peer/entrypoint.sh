#!/bin/bash
# Asegurar que el contenedor tenga privilegios para modificar iptables

echo "Configurando reglas de iptables..."

iptables -A INPUT -s 172.20.0.100 -j ACCEPT
iptables -A OUTPUT -d 172.20.0.100 -j ACCEPT

iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -s 172.20.0.0/24 -j ACCEPT
iptables -A INPUT -s 172.20.0.0/24 -j DROP


echo "Reglas de iptables configuradas."

# Mantener el contenedor en ejecución
exec "$@"