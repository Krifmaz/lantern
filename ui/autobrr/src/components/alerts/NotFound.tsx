/*
 * Copyright (c) 2021 - 2025, Ludvig Lundgren and the autobrr contributors.
 * SPDX-License-Identifier: GPL-2.0-or-later
 */

import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";

// LANTERN: the instance name in place of autobrr's logo (src/lantern/Wordmark.tsx)
import Logo from "@app/lantern/Wordmark";

export const NotFound = () => {
  const { t } = useTranslation("common");

  return (
    <div className="min-h-screen flex flex-col justify-center ">
      <div className="flex justify-center">
        <Logo className="h-24 sm:h-48"/>
      </div>
      <h2 className="text-2xl text-center font-bold text-gray-900 dark:text-gray-200 my-8 px-2">
        {t("notFound.status404")}
      </h2>
      <h1 className="text-3xl text-center font-bold text-gray-900 dark:text-gray-200 my-8 px-2">
        {t("notFound.title")}
      </h1>
      {/* LANTERN: no links to autobrr's GitHub and Discord; Lantern has its own bug reports */}
      <h3 className="text-xl text-center leading-6 text-gray-700 dark:text-gray-400 mb-8 px-2">
        {t("notFound.backOnTrack")}
      </h3>
      <div className="flex justify-center">
        <Link to="/">
          <button
            className="w-48 flex justify-center py-2 px-4 border border-transparent rounded-md shadow-xs text-sm font-medium text-white bg-blue-600 dark:bg-blue-600 hover:bg-blue-700 dark:hover:bg-blue-700 focus:outline-hidden focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 dark:focus:ring-blue-500 cursor-pointer"
          >
            {t("notFound.backToDashboard")}
          </button>
        </Link>
      </div>
    </div>
  );
};
