SAMPLE=$(ls -t *.raw |head -n1) # lastest raw file
echo Playing $SAMPLE

gst-launch-1.0 filesrc location=$SAMPLE ! \
capsfilter caps="audio/x-raw, format=S16LE, channels=1, rate=48000" ! \
autoaudiosink
