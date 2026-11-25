# The shell integration's `ssh` runs this with the file of the remote start command first, then ssh's own arguments.
# Only a session that opens a shell on the other host gets the integration there (F89); any other use of ssh, such as
# a remote command, a tunnel or git, runs unchanged.

remote=$1
shift

if [ ! -t 0 ] || [ ! -t 1 ] || [ ! -r "$remote" ]; then
  exec ssh "$@"
fi

# ssh's options that take a value, as its getopt string lists them.
with_value=BbcDEeFIiJLlmOoPpQRSWw
# Options after which ssh opens no shell: no command, no terminal, a forwarding only, a control or query command.
no_shell=NTWOGVQfs

pending=
destination=
ended=
for arg do
  if [ -n "$pending" ]; then
    pending=
    continue
  fi
  if [ -n "$ended" ]; then
    [ -z "$destination" ] || exec ssh "$@"
    destination=$arg
    continue
  fi
  case $arg in
    --)
      ended=1
      ;;
    -?*)
      rest=${arg#-}
      while [ -n "$rest" ]; do
        flag=${rest%"${rest#?}"}
        rest=${rest#?}
        case $no_shell in
          *"$flag"*) exec ssh "$@" ;;
        esac
        case $with_value in
          *"$flag"*)
            [ -n "$rest" ] || pending=1
            rest=
            ;;
        esac
      done
      ;;
    *)
      # ssh reads options after the destination too; the first other word begins a remote command.
      [ -z "$destination" ] || exec ssh "$@"
      destination=$arg
      ;;
  esac
done

if [ -z "$destination" ] || [ -n "$pending" ]; then
  exec ssh "$@"
fi

newline='
'
config=$newline$(ssh -G "$@" 2>/dev/null) || exec ssh "$@"
case $config in
  *"${newline}remotecommand "* | *"${newline}requesttty no"* | *"${newline}sessiontype none"* | *"${newline}sessiontype subsystem"*)
    exec ssh "$@"
    ;;
esac

exec ssh -t "$@" "$(cat "$remote")"
