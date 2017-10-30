SAMPLE=$1 # arg
echo Playing $SAMPLE

GST_DEBUG_DUMP_DOT_DIR=dot/ gst-launch-1.0 osxaudiosrc ! \
audioresample ! \
capsfilter caps="audio/x-raw, format=S16LE, channels=1, rate=16000" ! \
autoaudiosink

# dot -Tpng dot/0.00.01.467813000-gst-launch.PAUSED_PLAYING.dot > pipeline.png