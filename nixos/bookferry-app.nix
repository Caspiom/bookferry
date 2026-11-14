# Instala o Bookferry desktop no sistema (aparece no launcher).
# Importe a partir da checkout do repositorio, nao de uma copia em /etc/nixos, porque o pacote
# compila o codigo que esta ao lado deste arquivo:
#   imports = [ /home/gappy/Documents/projetos/bookshelf/nixos/bookferry-app.nix ];
{ pkgs, ... }:
{
  environment.systemPackages = [ (pkgs.callPackage ./bookferry.nix { }) ];
}
