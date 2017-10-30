SAMPLE=$1 # arg
echo Playing $SAMPLE

GST_DEBUG_DUMP_DOT_DIR=dot/ gst-launch-1.0 filesrc location=$SAMPLE ! \
capsfilter caps="audio/x-raw, format=S16LE, channels=1, rate=16000" ! \
autoaudiosink

# dot -Tpng dot/0.00.07.057477000-gst-launch.PLAYING_PAUSED.dot > pipeline-file.png