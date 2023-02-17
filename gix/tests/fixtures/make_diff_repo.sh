#!/bin/bash
set -eu -o pipefail

git init -q

git checkout -b main
mkdir dir
touch a b dir/c d
git add .
git commit -q -m c1

echo a >> a
echo b >> b
echo dir/c >> dir/c
echo d >> d
git commit -q -am c2

echo a1 >> a
git commit -q -am c3

git mv a dir/a-moved
git commit -m r1-identity

touch s1 s2 s3
git add s* && git commit -m "c4 - add identical files"

git mv s1 z && git mv s2 b2 && git mv s3 b1
git commit -m r2-ambiguous

git mv dir/c dir/c-moved
echo n >> dir/c-moved
echo n >> b

git commit -am "r3" # modified rename and normal modification

