package main

import (
	"context"
	"flag"
	"os"
	"os/signal"
	"syscall"

	"github.com/termforge/forge/internal/config"
	"github.com/termforge/forge/internal/daemon"
	"github.com/termforge/forge/internal/storage"
	"github.com/rs/zerolog"
)

func main() {
	cfgPath := flag.String("config", "", "path to config file")
	dataDir := flag.String("data-dir", "", "path to data directory")
	flag.Parse()

	log := zerolog.New(zerolog.ConsoleWriter{Out: os.Stderr}).
		With().Timestamp().Logger()

	cfg, err := config.Load(*cfgPath, *dataDir)
	if err != nil {
		log.Fatal().Err(err).Msg("failed to load config")
	}

	db, err := storage.NewSQLite(cfg.DataDir)
	if err != nil {
		log.Fatal().Err(err).Msg("failed to open database")
	}
	defer db.Close()

	if err := db.RunMigrations(); err != nil {
		log.Fatal().Err(err).Msg("failed to run migrations")
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	d, err := daemon.New(daemon.Opts{
		Config:  cfg,
		DB:      db,
		Logger:  log,
	})
	if err != nil {
		log.Fatal().Err(err).Msg("failed to create daemon")
	}

	log.Info().Str("addr", cfg.ListenAddr).Msg("forged daemon starting")

	if err := d.Run(ctx); err != nil {
		log.Fatal().Err(err).Msg("daemon exited with error")
	}

	log.Info().Msg("forged daemon stopped")
}
